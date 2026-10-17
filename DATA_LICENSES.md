# Data and model licenses

The source code is MIT (see `LICENSE`). The trained model, `typethai.ttm`, is shipped as a release asset and inside
the app packages. It is **not** in this repository. It holds only derived statistics: character trigram counts,
word lists, word frequencies, word pair counts, and logistic-regression weights. The reverse-direction weights are in
`rust/crates/typethai-core/src/reverse-model.json`. The model contains no training text, apart from the word lists below.

## Sources used to build the shipped model

| Source | Used for | License |
|---|---|---|
| Thai Wikipedia: random articles, plain-text extracts via the MediaWiki API | Thai character n-grams, word frequencies, training lines for the classifiers | CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/). Text © Wikipedia contributors |
| English Wikipedia: random articles | English character n-grams, the English word list (words seen at least 3 times), classifier negatives | CC BY-SA 4.0. Text © Wikipedia contributors |
| PyThaiNLP `words_th.txt` (62,107 words), https://github.com/PyThaiNLP/pythainlp | Thai dictionary, typo-suggestion candidates (included verbatim in the model) | CC0 1.0 (PyThaiNLP `corpus_license.md`) |
| Wisesight Sentiment Corpus, https://github.com/PyThaiNLP/wisesight-sentiment | Chat-style Thai: n-grams, word frequencies, classifier training, a mined list of Latin brands/abbreviations | CC0 1.0 |
| FrequencyWords `en_50k` (OpenSubtitles 2018), https://github.com/hermitdave/FrequencyWords | English word frequencies for word completion (section 11, 46,676 words) | CC BY-SA 4.0 |
| scb-mt-en-th-2020 v1.0 (AIResearch / VISTEC / SCB), https://github.com/vistec-AI/dataset-releases/releases/tag/scb-mt-en-th-2020_v1.0. Only the `task_master_1`, `generated_reviews_translator`, `generated_reviews_crowd` and `mozilla_common_voice` subsets | Thai and English word pair counts (sections 12, 13). The English side of `task_master_1` is Google's Taskmaster-1 dialogs (CC BY 4.0); the English reviews are machine-generated and only their Thai translations are used | CC BY-SA 4.0; the Common Voice pairs CC0 1.0 |
| Tatoeba sentences, English and Thai per-language exports (weekly export, downloaded 2026-10-15), https://tatoeba.org/en/downloads | Word pair counts | CC BY 2.0 FR (part CC0 1.0). Sentences © Tatoeba contributors |
| VISTEC-TP-TH-2021 (VISTEC and Chiang Mai University), https://github.com/mrpeerat/OSKut/tree/main/VISTEC-TP-TH-2021. The raw tweets only; its segmentation and tags are not used | Thai word pair counts (chat-style Thai: slang, netspeak) | CC BY-SA 3.0 (adapted under CC BY-SA 4.0, as its section 4(b) allows). Text from Twitter, 2017–2019 |
| Wikipedia via the `wikimedia/wikipedia` 2023-11-01 dump (Hugging Face), one parquet shard per language | Word pair counts | CC BY-SA 4.0. Text © Wikipedia contributors |
| A hand-written brand list (`data/brands_hand.txt`) | Words to keep as English | MIT (this project) |
| Source code, lines without Thai | English character n-grams and "code" negatives for the classifier (aggregate counts and weights only; no code text or identifiers are in the model) | Used only as statistics. The original files are not distributed |

Because the model is derived from Wikipedia text, `typethai.ttm` is distributed under **CC BY-SA 4.0**. Attribution:
"Wikipedia contributors; PyThaiNLP; Wisesight; FrequencyWords (Hermit Dave); scb-mt-en-th-2020
(AIResearch, VISTEC, SCB), including Taskmaster-1 (Google LLC); Tatoeba contributors; VISTEC-TP-TH-2021 (VISTEC,
Chiang Mai University)". The code that uses it stays MIT.

## Bundled font

`rust/crates/typethai-desktop/fonts/typeThaiSans-*.ttf` (Regular, Medium, SemiBold, Bold) is built by
`scripts/fonts/build-typethai-sans.py` from **Noto Sans Thai** 2.002 (https://github.com/notofonts/thai) and the Latin
letters of **Noto Sans** 2.015 (https://github.com/notofonts/latin-greek-cyrillic), both © The Noto Project Authors under
the SIL Open Font License 1.1 (`rust/crates/typethai-desktop/fonts/OFL.txt`). It is renamed typeThai Sans because it is
a modified version, and it stays under the OFL.

## Bundled flags

`rust/crates/typethai-desktop/flags/th.svg` and `us.svg`, the square flags on the bubble's conversion badges, are from
**flag-icons** 7.2.3 (https://github.com/lipis/flag-icons), © Panayiotis Lipiridis, under the MIT License
(`rust/crates/typethai-desktop/flags/LICENSE`). The typo badge's spell-check icon is Lucide's (ISC), from the gpui-kit
icon catalog.

## Used for evaluation only (not in the model)

- **Wongnai restaurant reviews** (wongnai-corpus): LGPL-3.0. Used as a held-out test set.
- **Thai Toxicity Tweet Corpus** (tmu-nlp): CC BY-NC 3.0. **Non-commercial**, so it was used only as a held-out
  evaluation set. It is not in the shipped model or any training split. The training scripts read only the
  `train` splits (wiki and Wisesight), and `fresh/*` is read only by the evaluation commands.

The word pair sections hold only counts of adjacent dictionary or completion words, and no sentences.
`scripts/bigrams/` rebuilds them (`fetch.sh` lists the exact URLs and pinned revisions). The other scb-mt-en-th-2020
subsets were left out on purpose: `nus_sms` and `msr_paraphrase` translate corpora whose own terms were not confirmed as CC-compatible, and
`thai_websites`, `paracrawl`, `apdf` and `assorted_government` are third-party web and government text.

None of these datasets are redistributed here. The `scripts/` directory shows how to rebuild the splits from the
original sources.
