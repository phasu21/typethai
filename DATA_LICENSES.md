# Data and model licenses

The source code is MIT (see `LICENSE`). The trained model, `typethai.ttm`, is shipped as a release asset and inside
the app packages. It is **not** in this repository. It holds only derived statistics: character trigram counts,
word lists, word frequencies, and logistic-regression weights. The reverse-direction weights are in
`rust/crates/typethai-core/src/reverse-model.json`. The model contains no training text, apart from the word lists below.

## Sources used to build the shipped model

| Source | Used for | License |
|---|---|---|
| Thai Wikipedia: random articles, plain-text extracts via the MediaWiki API | Thai character n-grams, word frequencies, training lines for the classifiers | CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/). Text © Wikipedia contributors |
| English Wikipedia: random articles | English character n-grams, the English word list (words seen at least 3 times), classifier negatives | CC BY-SA 4.0. Text © Wikipedia contributors |
| PyThaiNLP `words_th.txt` (62,107 words), https://github.com/PyThaiNLP/pythainlp | Thai dictionary, typo-suggestion candidates (included verbatim in the model) | CC0 1.0 (PyThaiNLP `corpus_license.md`) |
| Wisesight Sentiment Corpus, https://github.com/PyThaiNLP/wisesight-sentiment | Chat-style Thai: n-grams, word frequencies, classifier training, a mined list of Latin brands/abbreviations | CC0 1.0 |
| FrequencyWords `en_50k` (OpenSubtitles 2018), https://github.com/hermitdave/FrequencyWords | English word frequencies for word completion (section 11, 46,692 words) | CC BY-SA 4.0 |
| A hand-written brand list (`data/brands_hand.txt`) | Words to keep as English | MIT (this project) |
| Source code, lines without Thai | English character n-grams and "code" negatives for the classifier (aggregate counts and weights only; no code text or identifiers are in the model) | Used only as statistics. The original files are not distributed |

Because the model is derived from Wikipedia text, `typethai.ttm` is distributed under **CC BY-SA 4.0**. Attribution:
"Wikipedia contributors; PyThaiNLP; Wisesight; FrequencyWords (Hermit Dave)". The code that uses it stays MIT.

## Used for evaluation only (not in the model)

- **Wongnai restaurant reviews** (wongnai-corpus): LGPL-3.0. Used as a held-out test set.
- **Thai Toxicity Tweet Corpus** (tmu-nlp): CC BY-NC 3.0. **Non-commercial**, so it was used only as a held-out
  evaluation set. It is not in the shipped model or any training split. The training scripts read only the
  `train` splits (wiki and Wisesight), and `fresh/*` is read only by the evaluation commands.

None of these datasets are redistributed here. The `scripts/` directory shows how to rebuild the splits from the
original sources.
