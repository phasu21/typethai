//! The contextual word predictor's thread: a personal word n-gram model that finishes the word being typed from the
//! words around it (`typethai-predict`, ported from oh-my-pi's `pi-predict`).
//!
//! The model lives here, off the key tap callback: the engine sends a finished line to learn from, asks once per word
//! state, and reads the answer on a later tick. Learning a line costs well under a millisecond, but restoring the
//! snapshot and writing it cost milliseconds, and a slow tap callback gets the tap disabled — the same reason
//! `words.json` is written by the saver thread.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use typethai_predict::{Config, Predictor, Query, WordOracle};

/// How long after the last learned line the snapshot is written.
const PERSIST_DEBOUNCE: Duration = Duration::from_secs(300);
/// ...and the longest a write is put off while lines keep arriving.
const PERSIST_MAX_DIRTY: Duration = Duration::from_secs(900);
/// How often the loop looks at the clock with nothing to do.
const TICK: Duration = Duration::from_secs(5);

enum Cmd {
    /// Learn a finished line.
    Learn(String),
    /// Ask for the rest of `prefix`, typed after `before`.
    Ask { key: u64, before: String, prefix: String },
    /// Drop everything learned and start over.
    Forget,
    /// Write the snapshot and answer when it is on disk.
    Flush(mpsc::Sender<()>),
}

/// What the thread sends back.
enum Reply { Answer { key: u64, suffix: String, confidence: f64 } }

/// The predictor's handle: cheap to send to, never blocks.
pub struct Predict { tx: mpsc::Sender<Cmd>, rx: mpsc::Receiver<Reply> }

impl Predict {
    /// Start the thread; the model is restored from `state_dir` there.
    pub fn spawn(state_dir: PathBuf, oracle: Arc<dyn WordOracle>) -> Predict {
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        std::thread::Builder::new().name("typethai-predict".into())
            .spawn(move || run(&state_dir, &oracle, &jobs, &out))
            .expect("spawn the predictor thread");
        Predict { tx, rx }
    }
    pub fn learn(&self, line: String) { let _ = self.tx.send(Cmd::Learn(line)); }
    pub fn ask(&self, key: u64, before: String, prefix: String) { let _ = self.tx.send(Cmd::Ask { key, before, prefix }); }
    pub fn forget(&self) { let _ = self.tx.send(Cmd::Forget); }
    /// The next answer the model gave, if one is waiting: the query it belongs to, the letters it appends, and how
    /// sure it is.
    pub fn answer(&self) -> Option<(u64, String, f64)> {
        match self.rx.try_recv() { Ok(Reply::Answer { key, suffix, confidence }) => Some((key, suffix, confidence)), Err(_) => None }
    }
    /// Write the snapshot, and return once it is on disk (quitting).
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.tx.send(Cmd::Flush(done)).is_ok() { let _ = wait.recv(); }
    }
}

fn run(dir: &Path, oracle: &Arc<dyn WordOracle>, jobs: &mpsc::Receiver<Cmd>, out: &mpsc::Sender<Reply>) {
    let mut engine = open(dir, oracle);
    let (mut first, mut last): (Option<Instant>, Option<Instant>) = (None, None);
    loop {
        match jobs.recv_timeout(TICK) {
            Ok(Cmd::Learn(line)) => {
                if let Some(e) = &mut engine { e.observe(&line); first.get_or_insert_with(Instant::now); last = Some(Instant::now()); }
            }
            Ok(Cmd::Ask { key, before, prefix }) => {
                if let Some(hint) = engine.as_mut().and_then(|e| e.complete(&Query { before: &before, prefix: &prefix })) {
                    let _ = out.send(Reply::Answer { key, suffix: hint.suffix, confidence: f64::from(hint.confidence) });
                }
            }
            Ok(Cmd::Forget) => {
                wipe(dir);
                engine = open(dir, oracle);
                (first, last) = (None, None);
            }
            Ok(Cmd::Flush(done)) => { persist(&mut engine, &mut first, &mut last); let _ = done.send(()); }
            // the engine dropped the handle (the setting went off, or the app is quitting): keep what was learned
            Err(RecvTimeoutError::Disconnected) => { persist(&mut engine, &mut first, &mut last); return },
            Err(RecvTimeoutError::Timeout) => {},
        }
        let due = last.is_some_and(|t| t.elapsed() >= PERSIST_DEBOUNCE) || first.is_some_and(|t| t.elapsed() >= PERSIST_MAX_DIRTY);
        if due { persist(&mut engine, &mut first, &mut last) }
    }
}

/// Write the snapshot, if anything was learned since the last one. A failure is reported and the words stay in
/// memory: the next write tries again.
fn persist(engine: &mut Option<Box<dyn Predictor>>, first: &mut Option<Instant>, last: &mut Option<Instant>) {
    let Some(e) = engine.as_mut() else { return };
    if first.is_none() { return }
    (*first, *last) = (None, None);
    if let Err(error) = e.persist() { eprintln!("typeThai: word predictor not saved ({error})") }
}

/// The model in `dir`; a snapshot from another version or a damaged one is dropped rather than blocking startup, which
/// only costs the words it held.
fn open(dir: &Path, oracle: &Arc<dyn WordOracle>) -> Option<Box<dyn Predictor>> {
    let config = Config { state_dir: dir.to_path_buf(), oracle: Some(Arc::clone(oracle)), show_threshold: None };
    match typethai_predict::open(&config) {
        Ok(e) => Some(e),
        Err(error) => {
            eprintln!("typeThai: word predictor state unusable ({error}); starting over");
            wipe(dir);
            typethai_predict::open(&config).map_err(|error| eprintln!("typeThai: word predictor off ({error})")).ok()
        }
    }
}

fn wipe(dir: &Path) { let _ = std::fs::remove_file(dir.join(typethai_predict::ngram::SNAPSHOT_FILE)); }

#[cfg(test)]
mod tests {
    use super::*;
    use typethai_core::predict::KnownWords;
    use typethai_core::dict::Dict;
    use std::collections::HashSet;

    fn oracle(known: &[&str]) -> Arc<dyn WordOracle> {
        Arc::new(KnownWords::new(Arc::new(Dict::new(known.iter().map(|w| w.to_string()))), Arc::new(HashSet::new())))
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("typethai-predict-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// The next answer the thread gives (it runs on its own thread, so wait for it).
    fn answer(p: &Predict) -> Option<(u64, String, f64)> {
        (0..200).find_map(|_| { std::thread::sleep(Duration::from_millis(10)); p.answer() })
    }

    /// The line the user finished is what the next word is predicted from, and it survives a restart.
    #[test]
    fn a_finished_line_predicts_the_next_word_and_survives_a_restart() {
        let d = dir("roundtrip");
        let p = Predict::spawn(d.clone(), oracle(&["parser"]));
        p.learn("please refactor the parser".to_string());
        p.ask(1, "please refactor the ".to_string(), "pars".to_string());
        let (key, suffix, confidence) = answer(&p).expect("an answer");
        assert_eq!(key, 1);
        assert_eq!(suffix, "er");
        assert!(confidence > 0.0, "confidence {confidence}");
        p.flush();

        let p = Predict::spawn(d.clone(), oracle(&["parser"]));
        p.ask(2, "please refactor the ".to_string(), "pars".to_string());
        let (key, suffix, _) = answer(&p).expect("the restored answer");
        assert_eq!((key, suffix.as_str()), (2, "er"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A word neither dictionary knows is offered only once the user typed it twice, as the model's own hygiene rules
    /// say; a word the dictionary knows is offered after one line.
    #[test]
    fn an_unknown_word_waits_for_its_second_line() {
        let d = dir("oov");
        let p = Predict::spawn(d.clone(), oracle(&[]));
        p.learn("ship the zqorbit".to_string());
        p.ask(1, "ship the ".to_string(), "zqor".to_string());
        assert!(answer(&p).is_none(), "a word typed once was offered");
        p.learn("ship the zqorbit".to_string());
        p.ask(2, "ship the ".to_string(), "zqor".to_string());
        let (key, suffix, _) = answer(&p).expect("an answer after the second line");
        assert_eq!((key, suffix.as_str()), (2, "bit"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Forget drops what was learned, on disk and in memory.
    #[test]
    fn forgetting_clears_the_model_on_disk() {
        let d = dir("forget");
        let p = Predict::spawn(d.clone(), oracle(&["parser"]));
        p.learn("please refactor the parser".to_string());
        p.flush();
        assert!(d.join(typethai_predict::ngram::SNAPSHOT_FILE).exists());
        p.forget();
        p.flush();
        p.ask(3, "please refactor the ".to_string(), "pars".to_string());
        let answer = answer(&p);
        assert!(answer.is_none(), "a forgotten model still answered {answer:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
