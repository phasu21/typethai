//! Minimal Rust port of the LATTE-WordSeg-Lite runtime (one line, no whitespace handling) to measure size and latency.
//! usage: latte-rs <assets dir> <unit: short|long> [--bench]   (lines on stdin -> tokens joined by '|')
use ort::{session::{Session, builder::GraphOptimizationLevel}, value::Tensor};
use std::{collections::HashSet, io::BufRead, time::Instant};

const N_CLASSES: i64 = 13;
const LEN_BUCKET: [u32; 17] = [0, 0, 0, 1, 2, 3, 3, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5];
fn no_start(c: char) -> bool { matches!(c as u32, 0x0E30..=0x0E3A | 0x0E45 | 0x0E47..=0x0E4E) }
fn leading(c: char) -> bool { matches!(c as u32, 0x0E40..=0x0E44) }

struct Latte { s: Session, table: Vec<u8>, vocab: std::collections::HashMap<char, i64>, lex: HashSet<String>, lens: Vec<usize> }
impl Latte {
    fn new(dir: &str) -> Self {
        let env = ort::environment::init().with_telemetry(false).build().unwrap();
        let s = Session::builder(&env).unwrap().with_optimization_level(GraphOptimizationLevel::Level3).unwrap()
            .with_intra_threads(1).unwrap().commit_from_file(format!("{dir}/model.int8.onnx")).unwrap();
        let table = std::fs::read(format!("{dir}/charclass.bin")).unwrap();
        let vocab = std::fs::read_to_string(format!("{dir}/vocab.txt")).unwrap().chars().enumerate().map(|(i, c)| (c, N_CLASSES + i as i64)).collect();
        let lex: HashSet<String> = std::fs::read_to_string(format!("{dir}/lexicon.txt")).unwrap().split('\n')
            .filter(|w| (2..=16).contains(&w.chars().count()) && !w.chars().any(char::is_whitespace)).map(String::from).collect();
        let mut lens: Vec<usize> = lex.iter().map(|w| w.chars().count()).collect(); lens.sort(); lens.dedup();
        Latte { s, table, vocab, lex, lens }
    }
    fn starts(&mut self, line: &str, unit: usize) -> Vec<bool> {
        let ch: Vec<char> = line.chars().collect(); let n = ch.len();
        let ids: Vec<i64> = ch.iter().map(|c| *self.vocab.get(c).unwrap_or(&(self.table[*c as usize] as i64))).collect();
        let mut bits = vec![0i64; n]; let mut cover = vec![0i64; n + 1];
        let mut buf = String::new();
        for i in 0..n { for &l in &self.lens { if i + l > n { break }
            buf.clear(); buf.extend(&ch[i..i + l]);
            if self.lex.contains(&buf) {
                let b = LEN_BUCKET[l.min(16)];
                bits[i] |= 1 << b; bits[i + l - 1] |= 1 << (6 + b);
                if l > 2 { cover[i + 1] += 1; cover[i + l - 1] -= 1 }
            }
        } }
        let mut acc = 0; for i in 0..n { acc += cover[i]; if acc > 0 { bits[i] |= 1 << 12 } }
        let out = self.s.run(ort::inputs![
            "ids" => Tensor::from_array(([1usize, n], ids)).unwrap(),
            "bits" => Tensor::from_array(([1usize, n], bits)).unwrap(),
            "lengths" => Tensor::from_array(([1usize], vec![n as i32])).unwrap()]).unwrap();
        let (_, lg) = out["logits"].try_extract_tensor::<f32>().unwrap();
        (0..n).map(|i| {
            let v = lg[i * 3..i * 3 + unit + 1].iter().cloned().fold(f32::MIN, f32::max);  // nested: max over coarser units
            i == 0 || (v >= 0.0 && !no_start(ch[i]) && !leading(ch[i - 1]))
        }).collect()
    }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let t0 = Instant::now();
    let mut m = Latte::new(&a[1]);
    let load = t0.elapsed();
    let unit = if a[2] == "long" { 0 } else { 1 };
    let lines: Vec<String> = std::io::stdin().lock().lines().map(|l| l.unwrap()).filter(|l| !l.is_empty()).collect();
    if a.iter().any(|x| x == "--bench") {
        for l in lines.iter().take(50) { m.starts(l, unit); } // warm up
        let mut ts: Vec<f64> = lines.iter().map(|l| { let t = Instant::now(); m.starts(l, unit); t.elapsed().as_secs_f64() * 1e6 }).collect();
        ts.sort_by(f64::total_cmp);
        let avg_len = lines.iter().map(|l| l.chars().count()).sum::<usize>() as f64 / lines.len() as f64;
        println!("load {:?} | n {} avg chars {:.1} | p50 {:.0}us p90 {:.0}us p99 {:.0}us max {:.0}us", load, ts.len(), avg_len, ts[ts.len() / 2], ts[ts.len() * 9 / 10], ts[ts.len() * 99 / 100], ts[ts.len() - 1]);
    } else {
        for l in &lines { let st = m.starts(l, unit); let mut o = String::new();
            for (c, s) in l.chars().zip(&st) { if *s && !o.is_empty() { o.push('|') } o.push(c) } println!("{o}"); }
    }
}
