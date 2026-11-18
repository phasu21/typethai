# Auto-convert when sure: false-conversion measurements

`typethai-desktop --eval-auto` with model-v4 (`typethai.ttm` from the 0.5.24 app), a fresh personal model, and no
user lists. Run on 2026-11-18. The Wikipedia splits that `--eval-live` reads are not on the build machine. This
evaluation builds its sets from the model's own word lists instead, plus the hand-written tricky lines in
`rust/crates/typethai-desktop/eval/`: short words, chat slang, names, brands, acronyms, code, URLs, e-mail addresses,
paths and everyday sentences.

How to read it:
- **FALSE** counts words the user meant as typed that would have been changed with no key pressed.
- **converted** is recall on text typed on the wrong layout.
- Conversion happens only at a word boundary (Space, Enter or punctuation).
- Thai is written without spaces, so the "two-word runs" and "chat lines" sets are closest to real typing.

## Chosen defaults

| Level | Forward | Reverse | Min keys | False on 23,408 English-layout words | False on 22,658 Thai-layout Thai words | Thai runs caught | English on Thai layout caught |
|---|---|---|---|---|---|---|---|
| **Cautious** (default) | p > 0.995 | p > 0.995, known English word | 4 | **0** | 0 | 96.4% | 51.7% |
| Balanced | p > 0.99 | p > 0.98, known English word | 4 | 1 (`aviv`) | 0 | 97.9% | 73.6% |
| Eager | p > 0.98 | p > 0.95, known English word | 3 | 8 (`dvd duh lulu ryo siu zhu aviv wfh`) | 0 | 98.7% | 85.7% |

A first version also required the Thai reading to be a dictionary word. That cut recall on unspaced runs to 0.2%,
and length plus probability already keep false conversions at zero (see the `dict yes/no` rows), so no level uses it.

## Full output

```
--eval-auto  model=/Applications/typeThai.app/Contents/Resources/typethai.ttm
  English top words (lines of 8)                  20000 words  (English layout, must not convert)
  English top 3,000, one per line                  3000 words  (English layout, must not convert)
  Tricky English (eval/auto-english.txt)            408 words  (English layout, must not convert)
  Thai top words as keys (lines of 6)             19633 words  (English layout, should convert)
  Thai two-word runs as keys                       3000 words  (English layout, should convert)
  Thai chat lines as keys (eval/auto-thai.txt)       25 words  (English layout, should convert)
  Thai top words on the Thai layout               19633 words  (Thai layout, must not convert)
  Thai two-word runs on the Thai layout            3000 words  (Thai layout, must not convert)
  Thai chat lines on the Thai layout                 25 words  (Thai layout, must not convert)
  English top words on the Thai layout            19999 words  (Thai layout, should convert)

## Levels (through the engine)

Cautious: forward p > 0.995, reverse p > 0.995, at least 4 keys, any word
  English top words (lines of 8)                 FALSE     0 /  20000 =   0.000%
  English top 3,000, one per line                FALSE     0 /   3000 =   0.000%
  Tricky English (eval/auto-english.txt)         FALSE     0 /    408 =   0.000%
  Thai top words as keys (lines of 6)            converted 16193 /  19633 =  82.478%
  Thai two-word runs as keys                     converted  2892 /   3000 =  96.400%
  Thai chat lines as keys (eval/auto-thai.txt)   converted    23 /     25 =  92.000%
  Thai top words on the Thai layout              FALSE     0 /  19633 =   0.000%
  Thai two-word runs on the Thai layout          FALSE     0 /   3000 =   0.000%
  Thai chat lines on the Thai layout             FALSE     0 /     25 =   0.000%
  English top words on the Thai layout           converted 10333 /  19999 =  51.668%
Balanced: forward p > 0.99, reverse p > 0.98, at least 4 keys, any word
  English top words (lines of 8)                 FALSE     1 /  20000 =   0.005%  e.g. aviv
  English top 3,000, one per line                FALSE     0 /   3000 =   0.000%
  Tricky English (eval/auto-english.txt)         FALSE     0 /    408 =   0.000%
  Thai top words as keys (lines of 6)            converted 16615 /  19633 =  84.628%
  Thai two-word runs as keys                     converted  2938 /   3000 =  97.933%
  Thai chat lines as keys (eval/auto-thai.txt)   converted    24 /     25 =  96.000%
  Thai top words on the Thai layout              FALSE     0 /  19633 =   0.000%
  Thai two-word runs on the Thai layout          FALSE     0 /   3000 =   0.000%
  Thai chat lines on the Thai layout             FALSE     0 /     25 =   0.000%
  English top words on the Thai layout           converted 14728 /  19999 =  73.644%
Eager: forward p > 0.98, reverse p > 0.95, at least 3 keys, any word
  English top words (lines of 8)                 FALSE     7 /  20000 =   0.035%  e.g. dvd duh lulu ryo siu zhu aviv
  English top 3,000, one per line                FALSE     0 /   3000 =   0.000%
  Tricky English (eval/auto-english.txt)         FALSE     1 /    408 =   0.245%  e.g. wfh
  Thai top words as keys (lines of 6)            converted 18695 /  19633 =  95.222%
  Thai two-word runs as keys                     converted  2961 /   3000 =  98.700%
  Thai chat lines as keys (eval/auto-thai.txt)   converted    24 /     25 =  96.000%
  Thai top words on the Thai layout              FALSE     0 /  19633 =   0.000%
  Thai two-word runs on the Thai layout          FALSE     0 /   3000 =   0.000%
  Thai chat lines on the Thai layout             FALSE     0 /     25 =   0.000%
  English top words on the Thai layout           converted 17142 /  19999 =  85.714%

## Grid (false conversions per 10,000 words | share converted)

thr    keys dict | N0 N1 N2 N6 N7 N8 | P3 P4 P5 P9
  N0 = English top words (lines of 8)
  N1 = English top 3,000, one per line
  N2 = Tricky English (eval/auto-english.txt)
  P3 = Thai top words as keys (lines of 6)
  P4 = Thai two-word runs as keys
  P5 = Thai chat lines as keys (eval/auto-thai.txt)
  N6 = Thai top words on the Thai layout
  N7 = Thai two-word runs on the Thai layout
  N8 = Thai chat lines on the Thai layout
  P9 = English top words on the Thai layout
0.9       2  yes |   13.0    6.7   73.5    0.0    0.0    0.0 |  98.7%   0.2%   8.0%  91.5%
0.9       2   no |   30.0    6.7  122.5    0.0    0.0    0.0 |  98.7%  99.4%  96.0%  91.5%
0.9       3  yes |    2.5    0.0   24.5    0.0    0.0    0.0 |  96.8%   0.2%   8.0%  90.9%
0.9       3   no |   18.5    0.0   73.5    0.0    0.0    0.0 |  96.8%  99.4%  96.0%  90.9%
0.9       4  yes |    0.0    0.0    0.0    0.0    0.0    0.0 |  86.6%   0.2%   8.0%  87.6%
0.9       4   no |   13.5    0.0   24.5    0.0    0.0    0.0 |  86.6%  99.4%  96.0%  87.6%
0.95      2  yes |    8.5    0.0   49.0    0.0    0.0    0.0 |  98.2%   0.2%   8.0%  86.2%
0.95      2   no |   14.5    0.0   98.0    0.0    0.0    0.0 |  98.2%  99.2%  96.0%  86.2%
0.95      3  yes |    2.5    0.0   24.5    0.0    0.0    0.0 |  96.4%   0.2%   8.0%  85.7%
0.95      3   no |    8.0    0.0   73.5    0.0    0.0    0.0 |  96.4%  99.2%  96.0%  85.7%
0.95      4  yes |    0.0    0.0    0.0    0.0    0.0    0.0 |  86.4%   0.2%   8.0%  82.7%
0.95      4   no |    4.5    0.0   24.5    0.0    0.0    0.0 |  86.4%  99.2%  96.0%  82.7%
0.98      2  yes |    4.5    0.0   49.0    0.0    0.0    0.0 |  96.8%   0.2%   8.0%  76.4%
0.98      2   no |    7.0    0.0   49.0    0.0    0.0    0.0 |  96.8%  98.7%  96.0%  76.4%
0.98      3  yes |    1.5    0.0   24.5    0.0    0.0    0.0 |  95.2%   0.2%   8.0%  76.0%
0.98      3   no |    3.5    0.0   24.5    0.0    0.0    0.0 |  95.2%  98.7%  96.0%  76.0%
0.98      4  yes |    0.0    0.0    0.0    0.0    0.0    0.0 |  85.7%   0.2%   8.0%  73.6%
0.98      4   no |    1.0    0.0    0.0    0.0    0.0    0.0 |  85.7%  98.7%  96.0%  73.6%
0.99      2  yes |    3.5    0.0   49.0    0.0    0.0    0.0 |  95.1%   0.2%   8.0%  67.0%
0.99      2   no |    5.0    0.0   49.0    0.0    0.0    0.0 |  95.1%  97.9%  96.0%  67.0%
0.99      3  yes |    0.5    0.0   24.5    0.0    0.0    0.0 |  93.6%   0.2%   8.0%  66.7%
0.99      3   no |    1.5    0.0   24.5    0.0    0.0    0.0 |  93.6%  97.9%  96.0%  66.7%
0.99      4  yes |    0.0    0.0    0.0    0.0    0.0    0.0 |  84.6%   0.2%   8.0%  64.8%
0.99      4   no |    0.5    0.0    0.0    0.0    0.0    0.0 |  84.6%  97.9%  96.0%  64.8%
0.995     2  yes |    2.0    0.0   49.0    0.0    0.0    0.0 |  91.9%   0.2%   8.0%  53.2%
0.995     2   no |    2.0    0.0   49.0    0.0    0.0    0.0 |  91.9%  96.4%  92.0%  53.2%
0.995     3  yes |    0.5    0.0   24.5    0.0    0.0    0.0 |  90.7%   0.2%   8.0%  53.0%
0.995     3   no |    0.5    0.0   24.5    0.0    0.0    0.0 |  90.7%  96.4%  92.0%  53.0%
0.995     4  yes |    0.0    0.0    0.0    0.0    0.0    0.0 |  82.5%   0.2%   8.0%  51.7%
0.995     4   no |    0.0    0.0    0.0    0.0    0.0    0.0 |  82.5%  96.4%  92.0%  51.7%
0.999     2  yes |    0.0    0.0   24.5    0.0    0.0    0.0 |  77.6%   0.1%   4.0%  17.5%
0.999     2   no |    0.0    0.0   24.5    0.0    0.0    0.0 |  77.6%  84.8%  72.0%  17.5%
0.999     3  yes |    0.0    0.0   24.5    0.0    0.0    0.0 |  77.0%   0.1%   4.0%  17.4%
0.999     3   no |    0.0    0.0   24.5    0.0    0.0    0.0 |  77.0%  84.8%  72.0%  17.4%
0.999     4  yes |    0.0    0.0    0.0    0.0    0.0    0.0 |  71.6%   0.1%   4.0%  17.1%
0.999     4   no |    0.0    0.0    0.0    0.0    0.0    0.0 |  71.6%  84.8%  72.0%  17.1%
```
