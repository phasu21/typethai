//! typethai-core: Rust port of the typeThai engine (see the TS sources in ../../src for the reference implementation).
pub mod keymap; pub mod orthography; pub mod normalize; pub mod ngram; pub mod dict; pub mod detector;
pub mod classifier; pub mod corrector; pub mod slip; pub mod personal; pub mod model; pub mod reverse; pub mod completer; pub mod corpus; pub mod predict;
pub use detector::{Detector, Params, TokenDecision};
pub use model::ModelFiles;
