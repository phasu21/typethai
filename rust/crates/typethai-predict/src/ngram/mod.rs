//! Personal word n-gram engine: the cheap, instantly available engine.
//!
//! A port of `pi-predict`'s n-gram engine (oh-my-pi,
//! `https://github.com/can1357/oh-my-pi`, tag `v18.11.3`, MIT — see
//! `LICENSE-THIRD-PARTY.md`): an interpolated absolute-discounting word
//! trigram model over the user's prose history, ranked under the typed prefix
//! with a finished-word-aware posterior, a prompt-local cache, a session
//! cache, and vocabulary hygiene. The origin's embedded Norvig web
//! unigram/bigram prior is gone; which spellings count as real words comes
//! from [`crate::WordOracle`], and the vocabulary holds only the sentence
//! start plus what the user has typed.
//!
//! # Model
//! - **Counts** (`model.rs`): prose words only (the editor's gates,
//!   [`crate::prose`]), with their previous one or two prose words as context.
//!   Prompts that look typed count 1, pastes and logs count
//!   [`Params::paste_weight`].
//! - **Ranking** (`query.rs`): word ids are lexicographic ranks, so a prefix is
//!   one id range; a Fenwick tree gives its unigram mass, a max segment tree
//!   its unigram top-k, id-sorted follower lists its bigram counts, and an
//!   open-addressing table its trigram counts.
//! - **Confidence**: posterior of the best word among *every* word under the
//!   prefix, including the prefix itself (`the|re` shows only when *there*
//!   clearly beats stopping at *the*), mixed with the prompt-local and session
//!   caches. Typing more letters of the shown word keeps it the pick: ghosts do
//!   not change under the user's fingers.
//! - **Hygiene**: words outside the oracle's vocabulary need
//!   [`Params::min_count`] typed occurrences; rare misspellings fold into an
//!   edit-distance-1 real word at learn time (not across apostrophes, not at
//!   the first or last letter); habitual slips of a word the user also types
//!   are never offered; pastes never make a word suggestible.
//!
//! # Operating point
//! Prefixes may be a single letter (`figure o|ut`); the rest of the word is
//! ranked with the same model, where left context does most of the work.
//! One show threshold τ = 0.15 ([`Params::show_threshold`]) gates every
//! prefix length; [`crate::Config::show_threshold`] replaces it. The point
//! favours recall: a wrong ghost costs a glance, a missing right one costs
//! the keystrokes. The origin's replay figures (keystrokes saved, wrong
//! ghosts, µs costs, heap) were measured against its web prior and do not
//! carry over to a personal-only vocabulary.
//!
//! # Persistence
//! [`Predictor::persist`] writes a versioned zstd snapshot
//! (`snapshot.rs`) to `ngram.snapshot` in [`crate::Config::state_dir`]
//! atomically (temp file + rename); [`open`] restores it and rejects an
//! unknown version or a corrupt file so the caller can wipe the directory
//! and re-ingest history.

mod hygiene;
mod model;
mod query;
mod snapshot;
mod structures;
#[cfg(test)]
mod tests;
mod text;
mod vocab;

use std::{
	io::Write,
	path::{Path, PathBuf},
};

use anyhow::Context;
pub use model::Params;

use crate::{Config, Predictor, Query, Suggestion};

/// Snapshot file inside the state directory.
pub const SNAPSHOT_FILE: &str = "ngram.snapshot";

/// The n-gram engine.
pub struct NgramPredictor {
	model:     model::Model,
	query:     query::QueryState,
	/// Caller override of [`Params::show_threshold`] (see
	/// [`Config::show_threshold`]).
	gate:      Option<f32>,
	state_dir: PathBuf,
}

/// Open the n-gram engine with the tuned [`Params`], restoring persisted
/// state from `config.state_dir`.
///
/// # Errors
/// Returns an error when the snapshot exists but cannot be read, has an
/// unknown version, or is corrupt.
pub fn open(config: &Config) -> anyhow::Result<Box<dyn Predictor>> {
	Ok(Box::new(NgramPredictor::open(config, Params::default())?))
}

impl NgramPredictor {
	/// Open with explicit `params` (tuning and benchmarks), restoring
	/// persisted state from `config.state_dir`.
	///
	/// # Errors
	/// Returns an error when the snapshot exists but cannot be read, has an
	/// unknown version, or is corrupt.
	pub fn open(config: &Config, params: Params) -> anyhow::Result<Self> {
		let gate = config.show_threshold;
		let path = config.state_dir.join(SNAPSHOT_FILE);
		let model = match std::fs::read(&path) {
			Ok(bytes) => snapshot::decode(&bytes, params, config.oracle.clone())
				.with_context(|| format!("restore {}", path.display()))?,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				model::Model::new(params, config.oracle.clone())
			},
			Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
		};
		Ok(Self {
			model,
			query: query::QueryState::default(),
			gate,
			state_dir: config.state_dir.clone(),
		})
	}

	/// Heap bytes of the learned model and query scratch.
	pub fn heap_bytes(&self) -> usize {
		self.model.heap_bytes() + self.query.heap_bytes()
	}
}

/// Write `bytes` to `dir/name` atomically: a temp file in the same directory,
/// then a rename over the target.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
	std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
	let target = dir.join(name);
	let temp = dir.join(format!("{name}.{}.tmp", std::process::id()));
	let written = (|| {
		let mut file = std::fs::File::create(&temp)?;
		file.write_all(bytes)?;
		file.sync_all()?;
		drop(file);
		std::fs::rename(&temp, &target)
	})();
	if let Err(error) = written {
		let _ = std::fs::remove_file(&temp);
		return Err(error).with_context(|| format!("write {}", target.display()));
	}
	Ok(())
}

impl Predictor for NgramPredictor {
	fn complete(&mut self, query: &Query<'_>) -> Option<Suggestion> {
		let (suffix, confidence) = self
			.query
			.complete(&self.model, query.before, query.prefix)?;
		let confidence = confidence as f32;
		let gate = self.gate.unwrap_or(self.model.params.show_threshold);
		(confidence >= gate).then_some(Suggestion { suffix, confidence })
	}

	fn next_word(&mut self, before: &str) -> Option<Suggestion> {
		let (word, confidence) = self.query.next_word(&self.model, before)?;
		let confidence = confidence as f32;
		let gate = self.gate.unwrap_or(self.model.params.show_threshold);
		(confidence >= gate).then_some(Suggestion { suffix: word, confidence })
	}

	fn observe(&mut self, prompt: &str) {
		self.model.learn(prompt);
	}

	fn feedback(&mut self, _query: &Query<'_>, _suggestion: &str, _accepted: bool) {
		// Accepted words are learned when the prompt is submitted. A calibrated
		// per-(prefix, word) feedback bias measured no gain.
	}

	fn persist(&mut self) -> anyhow::Result<()> {
		self.model.prune_trigrams();
		let bytes = snapshot::encode(&self.model)?;
		write_atomic(&self.state_dir, SNAPSHOT_FILE, &bytes)
	}
}
