//! Ghost-text word completion for the typeThai composer.
//!
//! The engine answers "what is the rest of the word being typed?" for one
//! [`Query`], "what word comes next?" for a line ([`Predictor::next_word`]),
//! and learns from submitted prompts; only the personal word n-gram engine is
//! ported here. It is a port of `pi-predict` from oh-my-pi
//! (`https://github.com/can1357/oh-my-pi`, `crates/pi-predict` at tag
//! `v18.11.3`, MIT — see `LICENSE-THIRD-PARTY.md`) with the embedded Norvig
//! web prior removed and word trust injected through [`WordOracle`].
//!
//! # Example
//! ```ignore
//! let mut engine = typethai_predict::open(&Config::default())?;
//! engine.observe("can you refactor the parser");
//! let hint = engine.complete(&Query { before: "can you ", prefix: "re" });
//! ```

use std::{fmt, path::PathBuf, sync::Arc};

pub mod ngram;
pub mod prose;

pub use ngram::Params;

/// Editor state when ghost text is requested.
#[derive(Clone, Copy, Debug)]
pub struct Query<'a> {
	/// Prompt text before the word being typed (may span lines and contain
	/// code). The client may truncate it to a recent tail.
	pub before: &'a str,
	/// Letters of the current prose word typed so far (`[\p{L}\p{M}']+`),
	/// at least 2 characters; may already be a finished word.
	pub prefix: &'a str,
}

/// Ghost text to paint after [`Query::prefix`].
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
	/// Non-empty characters appended after the prefix.
	pub suffix:     String,
	/// Engine-calibrated probability that `suffix` is exactly right.
	pub confidence: f32,
}

/// Which spellings the engine treats as real words.
///
/// The origin took this from its embedded web prior and the system
/// dictionary; this crate asks the host instead. `known` is called whenever a
/// word is learned (and once per restored word), never on the completion path,
/// and it must be cheap and thread-safe.
pub trait WordOracle: Send + Sync {
	/// Is `word` a real spelling? `word` is a lowercased prose word
	/// (`[\p{L}\p{M}']+`, no code, digits, or camelCase).
	fn known(&self, word: &str) -> bool;
}

/// Construction options.
#[derive(Clone, Default)]
pub struct Config {
	/// Private directory the engine persists learned state into.
	pub state_dir:      PathBuf,
	/// Minimum confidence [`Predictor::complete`] returns; `None` = the
	/// engine's tuned default. Evaluation passes `f32::NEG_INFINITY` to trace
	/// the full curve.
	pub show_threshold: Option<f32>,
	/// Word trust (see [`WordOracle`]). Without an oracle nothing is trusted,
	/// so every word needs [`Params::min_count`] typed occurrences before it
	/// may be suggested.
	pub oracle:         Option<Arc<dyn WordOracle>>,
}

impl fmt::Debug for Config {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Config")
			.field("state_dir", &self.state_dir)
			.field("show_threshold", &self.show_threshold)
			.field("oracle", &self.oracle.as_ref().map(|_| "dyn WordOracle"))
			.finish()
	}
}

/// A word-completion engine. Called from one engine thread only.
pub trait Predictor: Send {
	/// Best completion for `query`, already gated by the show threshold, or
	/// `None` to show nothing. Must not change learned state; `&mut` exists
	/// only for inference caches (e.g. the per-`before` context memo).
	fn complete(&mut self, query: &Query<'_>) -> Option<Suggestion>;
	/// Learn from one submitted prompt (bootstrap history rows and live
	/// submissions go through this same path). Engines own their filtering of
	/// code, pastes, and misspellings.
	fn observe(&mut self, prompt: &str);
	/// Learn from a suggestion the user accepted (Tab) or typed past.
	fn feedback(&mut self, query: &Query<'_>, suggestion: &str, accepted: bool);
	/// The word likely to follow `before`, for the moment nothing of a word is typed yet: `suffix` is that whole word,
	/// already cased. `None` when the last word of `before` was never followed by anything worth offering, so the host
	/// can fall back to its own word lists (`complete` needs at least two letters of a word and cannot answer this).
	/// Like [`Predictor::complete`], it must not change learned state; `&mut` is for inference caches only.
	fn next_word(&mut self, before: &str) -> Option<Suggestion>;
	/// Flush learned state to [`Config::state_dir`].
	///
	/// # Errors
	/// Returns an error when the state directory cannot be written.
	fn persist(&mut self) -> anyhow::Result<()>;
}

/// Open (or restore from [`Config::state_dir`]) the n-gram engine.
///
/// # Errors
/// Returns an error when persisted state exists but cannot be read, has an
/// unknown version, or is corrupt.
pub fn open(config: &Config) -> anyhow::Result<Box<dyn Predictor>> {
	ngram::open(config)
}
