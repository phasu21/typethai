//! The OS English spell checker behind `engine::Speller`: NSSpellChecker on macOS; on Windows ISpellChecker for the
//! guesses and the text prediction (`Windows.Data.Text`) for the completions.
use crate::engine::Speller;

/// The system spell checker, or None where there is none (or it has no English).
pub fn system() -> Option<Box<dyn Speller + Send>> {
    #[cfg(target_os = "macos")] { Some(Box::new(mac::Mac)) }
    #[cfg(windows)] { win::Win::start().map(|w| Box::new(w) as Box<dyn Speller + Send>) }
    #[cfg(not(any(windows, target_os = "macos")))] { None }
}

#[cfg(target_os = "macos")]
mod mac {
    use crate::engine::Speller;
    use objc2_app_kit::NSSpellChecker;
    use objc2_foundation::{NSNotFound, NSRange, NSString};
    /// Uses the shared checker from the capture thread; only the checkSpelling / guesses / completions calls are made.
    pub struct Mac;
    impl Speller for Mac {
        fn guesses(&self, before: &str, word: &str) -> Option<Vec<String>> {
            objc2::rc::autoreleasepool(|_| {
                let sc = NSSpellChecker::sharedSpellChecker();
                let (w, lang) = (NSString::from_str(word), NSString::from_str("en"));
                // the word is checked alone: started mid-line (macOS 26) the check passes over a misspelled last word
                let r = unsafe { sc.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(&w, 0, Some(&lang), false, 0, std::ptr::null_mut()) };
                if r.location == NSNotFound as usize || r.length == 0 { return None }
                // the guesses are ranked with the line in front
                let s = NSString::from_str(&format!("{before}{word}"));
                let r = NSRange::new(before.encode_utf16().count() + r.location, r.length);
                let g = sc.guessesForWordRange_inString_language_inSpellDocumentWithTag(r, &s, Some(&lang), 0)?;
                Some(g.iter().map(|x| x.to_string()).collect())
            })
        }
        fn completions(&self, before: &str, prefix: &str) -> Vec<String> {
            objc2::rc::autoreleasepool(|_| {
                let sc = NSSpellChecker::sharedSpellChecker();
                let (s, lang) = (NSString::from_str(&format!("{before}{prefix}")), NSString::from_str("en"));
                let r = NSRange::new(before.encode_utf16().count(), prefix.encode_utf16().count());
                sc.completionsForPartialWordRange_inString_language_inSpellDocumentWithTag(r, &s, Some(&lang), 0)
                    .map(|g| g.iter().map(|x| x.to_string()).collect()).unwrap_or_default()
            })
        }
    }
}

#[cfg(windows)]
mod win {
    use crate::engine::Speller;
    use std::{collections::HashMap, sync::{mpsc, Arc, Mutex}, time::Duration};
    use windows::{core::HSTRING, Data::Text::{TextPredictionGenerator, TextPredictionOptions}, Win32::{Globalization::*, System::Com::*}};
    use windows_collections::IIterable;
    enum Req {
        Guess(String, mpsc::Sender<Option<Vec<String>>>),
        /// The words in front and the prefix.
        Complete(Vec<String>, String, mpsc::Sender<Vec<String>>),
    }
    /// How many words in front the prediction is given.
    const PREDICT_CONTEXT: usize = 4;
    /// How long the keyboard hook waits for an answer. The checker takes 2-20 ms on a word it does not know (gibberish
    /// above all) and the prediction about 3 ms: waiting for them on every pause and Space stalled fast typing. An answer
    /// that comes later is cached for the next ask; the rows go up without it this time.
    const WAIT: Duration = Duration::from_millis(4);
    /// Answers kept; past this the cache starts over (ponytail: no LRU, a few hundred words of typing refill it).
    const CACHE_MAX: usize = 512;
    #[derive(Default)]
    struct Cache { guesses: HashMap<String, Option<Vec<String>>>, completions: HashMap<String, Vec<String>> }
    /// The answer if it comes within `WAIT`. Polled with short sleeps: `recv_timeout` on Windows wakes on the ~15.6 ms
    /// system tick, so a 4 ms wait could take 16.
    fn wait<T>(rx: mpsc::Receiver<T>) -> Option<T> {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            match rx.try_recv() {
                Ok(v) => return Some(v),
                Err(mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_micros(250)),
                Err(_) => return None,
            }
        }
    }
    fn complete_key(prev: &[String], prefix: &str) -> String { format!("{}\u{1}{prefix}", prev.join(" ")) }
    /// COM stays on its own MTA worker (like the UIA worker): the keyboard hook thread only waits a moment for it.
    pub struct Win { tx: mpsc::Sender<Req>, cache: Arc<Mutex<Cache>> }
    impl Win {
        pub fn start() -> Option<Win> {
            let (tx, rx) = mpsc::channel::<Req>();
            let cache = Arc::new(Mutex::new(Cache::default()));
            let filled = Arc::clone(&cache);
            let (ok_tx, ok_rx) = mpsc::channel();
            std::thread::Builder::new().name("typethai-spell".into()).spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED).ok();
                let lang = HSTRING::from("en-US");
                let checker = CoCreateInstance::<_, ISpellCheckerFactory>(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()
                    .filter(|f| f.IsSupported(&lang).is_ok_and(|b| b.as_bool()))
                    .and_then(|f| f.CreateSpellChecker(&lang).ok());
                let _ = ok_tx.send(checker.is_some());
                let Some(checker) = checker else { return };
                // the first prediction takes ~20-90 ms: made now, not at the first pause
                let predictor = TextPredictionGenerator::Create(&lang).ok();
                if let Some(g) = &predictor { let _ = predict(g, &[], "a"); }
                while let Ok(first) = rx.recv() {
                    // only the newest ask of each kind: older ones were given up on while typing went on, and answering
                    // them all would queue up behind fast typing
                    let (mut guess, mut complete) = (None, None);
                    for req in std::iter::once(first).chain(rx.try_iter()) { match req { r @ Req::Guess(..) => guess = Some(r), r @ Req::Complete(..) => complete = Some(r) } }
                    for req in [guess, complete].into_iter().flatten() { match req {
                        Req::Guess(word, reply) => {
                            let g = check(&checker, &word);
                            let mut c = filled.lock().unwrap(); if c.guesses.len() >= CACHE_MAX { c.guesses.clear() } c.guesses.insert(word, g.clone()); drop(c);
                            let _ = reply.send(g);
                        }
                        Req::Complete(prev, prefix, reply) => {
                            let p = predictor.as_ref().and_then(|g| predict(g, &prev, &prefix).ok()).unwrap_or_default();
                            let mut c = filled.lock().unwrap(); if c.completions.len() >= CACHE_MAX { c.completions.clear() } c.completions.insert(complete_key(&prev, &prefix), p.clone()); drop(c);
                            let _ = reply.send(p);
                        }
                    } }
                }
            }).ok()?;
            ok_rx.recv().ok()?.then_some(Win { tx, cache })
        }
    }
    unsafe fn check(checker: &ISpellChecker, word: &str) -> Option<Vec<String>> {
        let errors = checker.Check(&HSTRING::from(word)).ok()?;
        let mut err = None;
        if errors.Next(&mut err).is_err() { return None }
        err?;
        let list = checker.Suggest(&HSTRING::from(word)).ok()?;
        let mut out = vec![];
        loop {
            let mut item = [windows::core::PWSTR::null()];
            if list.Next(&mut item, None).0 != 0 || item[0].is_null() { break }
            out.push(item[0].to_string().unwrap_or_default());
            CoTaskMemFree(Some(item[0].0 as _));
        }
        Some(out)
    }
    /// The words Windows' text prediction expects `prefix` to become after `prev`, best first. Predictions only: its
    /// corrections ignore the words in front, and mixed in they bring touch-keyboard misses (`hel` -> `gel`, `he m`).
    /// It answers only in a desktop session (in session 0 the call fails).
    fn predict(g: &TextPredictionGenerator, prev: &[String], prefix: &str) -> windows::core::Result<Vec<String>> {
        let prev: IIterable<HSTRING> = prev.iter().map(HSTRING::from).collect::<Vec<_>>().into();
        let list = g.GetCandidatesWithParametersAsync(&HSTRING::from(prefix), 4, TextPredictionOptions::Predictions, &prev)?.join()?;
        (0..list.Size()?).map(|i| list.GetAt(i).map(|s| s.to_string())).collect()
    }
    impl Speller for Win {
        // ISpellChecker reads one word at a time: the line in front is not used
        fn guesses(&self, _before: &str, word: &str) -> Option<Vec<String>> {
            if let Some(g) = self.cache.lock().unwrap().guesses.get(word) { return g.clone() }
            let (reply, rx) = mpsc::channel();
            self.tx.send(Req::Guess(word.to_string(), reply)).ok()?;
            wait(rx).flatten()
        }
        fn completions(&self, before: &str, prefix: &str) -> Vec<String> {
            let words: Vec<&str> = before.split_whitespace().collect();
            let prev: Vec<String> = words[words.len().saturating_sub(PREDICT_CONTEXT)..].iter().map(|w| w.to_string()).collect();
            if let Some(c) = self.cache.lock().unwrap().completions.get(&complete_key(&prev, prefix)) { return c.clone() }
            let (reply, rx) = mpsc::channel();
            if self.tx.send(Req::Complete(prev, prefix.to_string(), reply)).is_err() { return vec![] }
            wait(rx).unwrap_or_default()
        }
    }
}
