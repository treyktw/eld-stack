use eld_decoders_test::seq::SeqGen;

#[test]
fn maximum_boot_epoch_stays_inside_signed_bigint() {
    let mut seq = SeqGen::from_epoch(u64::MAX);
    assert!(seq.next() <= i64::MAX as u64);
}

#[test]
fn sequence_is_monotonic_within_a_boot() {
    let mut seq = SeqGen::from_epoch(42);
    let first = seq.next();
    assert_eq!(seq.next(), first + 1);
    assert_eq!(seq.latest(), Some(first + 1));
}

#[test]
fn a_later_persisted_boot_epoch_orders_after_the_prior_boot() {
    let mut earlier = SeqGen::from_epoch(100);
    let mut later = SeqGen::from_epoch(101);
    assert!(later.next() > earlier.next());
}
