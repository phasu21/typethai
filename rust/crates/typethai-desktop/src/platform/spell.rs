//! The OS English spell checker behind `engine::Speller`: NSSpellChecker on macOS, ISpellChecker on Windows.
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
        fn guesses(&self, word: &str) -> Option<Vec<String>> {
            objc2::rc::autoreleasepool(|_| {
                let sc = NSSpellChecker::sharedSpellChecker();
                let (s, lang) = (NSString::from_str(word), NSString::from_str("en"));
                let r = unsafe { sc.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(&s, 0, Some(&lang), false, 0, std::ptr::null_mut()) };
                if r.location == NSNotFound as usize || r.length == 0 { return None }
                let g = sc.guessesForWordRange_inString_language_inSpellDocumentWithTag(r, &s, Some(&lang), 0)?;
                Some(g.iter().map(|x| x.to_string()).collect())
            })
        }
        fn completions(&self, prefix: &str) -> Vec<String> {
            objc2::rc::autoreleasepool(|_| {
                let sc = NSSpellChecker::sharedSpellChecker();
                let (s, lang) = (NSString::from_str(prefix), NSString::from_str("en"));
                let r = NSRange::new(0, s.length());
                sc.completionsForPartialWordRange_inString_language_inSpellDocumentWithTag(r, &s, Some(&lang), 0)
                    .map(|g| g.iter().map(|x| x.to_string()).collect()).unwrap_or_default()
            })
        }
    }
}

#[cfg(windows)]
mod win {
    use crate::engine::Speller;
    use std::{sync::mpsc, time::Duration};
    use windows::{core::HSTRING, Win32::{Globalization::*, System::Com::*}};
    type Req = (String, mpsc::Sender<Option<Vec<String>>>);
    /// COM stays on its own MTA worker (like the UIA worker): the keyboard hook thread only waits a moment for it.
    pub struct Win { tx: mpsc::Sender<Req> }
    impl Win {
        pub fn start() -> Option<Win> {
            let (tx, rx) = mpsc::channel::<Req>();
            let (ok_tx, ok_rx) = mpsc::channel();
            std::thread::Builder::new().name("typethai-spell".into()).spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED).ok();
                let lang = HSTRING::from("en-US");
                let checker = CoCreateInstance::<_, ISpellCheckerFactory>(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()
                    .filter(|f| f.IsSupported(&lang).is_ok_and(|b| b.as_bool()))
                    .and_then(|f| f.CreateSpellChecker(&lang).ok());
                let _ = ok_tx.send(checker.is_some());
                let Some(checker) = checker else { return };
                for (word, reply) in rx { let _ = reply.send(check(&checker, &word)); }
            }).ok()?;
            ok_rx.recv().ok()?.then_some(Win { tx })
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
    impl Speller for Win {
        fn guesses(&self, word: &str) -> Option<Vec<String>> {
            let (reply, rx) = mpsc::channel();
            self.tx.send((word.to_string(), reply)).ok()?;
            rx.recv_timeout(Duration::from_millis(80)).ok().flatten()
        }
    }
}
