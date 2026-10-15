use typethai_core::keymap::{neighbours, reach, Reach};

#[test] fn keys_reach_their_neighbours() {
    assert_eq!(neighbours('d'), ['c', 'e', 'f', 'r', 's', 'x']);
    assert_eq!(neighbours('ก'), ['ด', 'ป', 'พ', 'ห', 'ำ', 'แ']);
    assert_eq!((reach('d', 'c'), reach('d', 'p'), reach('d', 'D')), (Reach::Near, Reach::Far, Reach::SameKey));
    assert_eq!((reach('า', 'ษ'), reach('า', 'ส'), reach('า', 'ฤ')), (Reach::SameKey, Reach::Near, Reach::Far));
}
