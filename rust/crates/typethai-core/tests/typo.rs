use typethai_core::keymap::{neighbours, reach, Reach};

#[test] fn keys_reach_their_neighbours() {
    assert_eq!(neighbours('d'), ['c', 'e', 'f', 'r', 's', 'x']);
    assert_eq!(neighbours('ก'), ['ด', 'ป', 'พ', 'ห', 'ำ', 'แ']);
    assert_eq!((reach('d', 'c'), reach('d', 'p'), reach('d', 'D')), (Reach::Near, Reach::Far, Reach::SameKey));
    assert_eq!((reach('า', 'ษ'), reach('า', 'ส'), reach('า', 'ฤ')), (Reach::SameKey, Reach::Near, Reach::Far));
}

use typethai_core::slip::{Slip, Slips};
fn close(a: f64, b: f64) -> bool { (a - b).abs() <= 1e-12 * b.abs().max(1e-300) }
#[test] fn slips_are_classified_by_key() {
    assert_eq!([Slip::instead('d', 'c'), Slip::instead('d', 'p'), Slip::instead('ก', 'ฏ')], [Slip::Near, Slip::Far, Slip::Shift]);
    assert_eq!([Slip::extra('l', Some('l'), Some('o')), Slip::extra('r', Some('h'), Some('e')), Slip::extra('m', Some('h'), Some('e'))], [Slip::Double, Slip::Extra, Slip::Stray]);
}
#[test] fn a_swap_is_likelier_than_a_neighbouring_key() {
    let s = Slips { rate: 0.01 };
    let cases = [("candel", "candle", 0.0015), ("candel", "cancel", 0.0005), ("helo", "hello", 0.0025), ("helllo", "hello", 0.0008), ("hrello", "hello", 0.0001), ("teh", "them", 0.0015 * 0.0025)];
    for (typed, meant, p) in cases { assert!(close(s.chance(typed, meant), p), "{typed} -> {meant}: {}", s.chance(typed, meant)) }
    assert_eq!(s.chance("the", "the"), 1.0);
}
