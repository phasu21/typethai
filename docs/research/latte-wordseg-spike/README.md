# LATTE spike tools

Scratch tools behind `../latte-wordseg-spike.md`, kept for reproducibility. They are not part of the build.

- `ttspike.rs`: a standalone crate (`typethai-core` as a path dependency). `seg` (benchmark predictions from
  `dict.segment`), `count` (Thai pair counts from dict, LATTE-pre-segmented or hybrid text, pruned like `--vocab`),
  `eval` (held-out next-word and completion hit rates, with views A and G), `runtime` (view R), `tail` (partial-word
  cases with `thai_tail`), `time`, `vocab`, `words`.
- `latte-rs.rs`: a minimal Rust port of the LATTE runtime on `ort = "=2.0.0-rc.15"`, used for parity, latency and
  size. Build it with `MACOSX_DEPLOYMENT_TARGET=13.4 RUSTFLAGS="-C link-arg=$(clang --print-resource-dir)/lib/darwin/libclang_rt.osx.a"`.
  It reads `model.int8.onnx`, `charclass.bin` (from `charclass.npz`), `vocab.txt` (from the ONNX metadata) and
  `lexicon.txt`.
- Python scripts (run them with `python -I` in a venv that has the wheel, numpy, onnxruntime and pyarrow):
  `extract_th.py` (`extract.py` with English removed), `latte_bench.py`, `latte_corpus.py`, `latte_tail2.py`,
  `agree.py`, and the formatters `fmt_ev.py` and `fmt_rt.py`.
- Raw results: `ev-all.txt` (views A and G), `rt-*.jsonl` (view R), `tail-latte2.json` (partial words).

The LATTE model, lexicon and corpora are not included. Download them from their pinned sources.
