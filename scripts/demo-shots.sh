#!/bin/bash
# Capture the bubble with 1/2/3 rows and a very long row (macOS; --demo-bubble --screenshot). Output: ${1:-./shots}
set -e; out=${1:-./shots}; mkdir -p "$out"; bin=${BIN:-./target/release/typethai-desktop}
"$bin" --demo-bubble --screenshot "$out/rows1.png" --items "สวัสดี  ← l;ylfu  (undo)" 2>&1 | grep -E "layout|viewport|ns frame|refit|screenshot"
"$bin" --demo-bubble --screenshot "$out/rows2.png" --items "สวัสดี  ← l;ylfu  (undo)|สวัสดีครับ (64%)" 2>&1 | grep -E "layout|viewport|ns frame|refit|screenshot"
"$bin" --demo-bubble --screenshot "$out/rows3.png" 2>&1 | grep -E "layout|viewport|ns frame|refit|screenshot"
"$bin" --demo-bubble --screenshot "$out/long.png" --items "สวัสดี (undo)|ภาษาไทย ทดสอบ วรรณยุกต์ ก่ ก้ ก๊ ก๋ ำ ฤๅษี ฦๅ ปฏิบัติ ญี่ปุ่น ฎีกา ฐาน ความยาวมากเกินไปจนต้องตัดด้วยจุดไข่ปลา (99%)|ok" 2>&1 | grep -E "layout|viewport|ns frame|refit|screenshot"
