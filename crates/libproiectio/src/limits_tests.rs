use super::*;

#[test]
fn the_default_bound_is_five_hundred_mebibytes() {
    assert_eq!(Limits::DEFAULT_MAX_SOURCE_BYTES, 524_288_000);
    assert_eq!(
        Limits::default().max_source_bytes,
        Limits::DEFAULT_MAX_SOURCE_BYTES
    );
}

#[test]
fn one_budget_is_spent_across_every_read() {
    let budget = Budget::new(Limits {
        max_source_bytes: 8,
    });

    assert_eq!(
        budget.read_to_end(&mut &b"12345"[..]).expect("read"),
        Some(b"12345".to_vec())
    );
    assert_eq!(budget.remaining(), 3);
    assert_eq!(budget.read_to_end(&mut &b"12345"[..]).expect("read"), None);
    assert!(budget.exhausted());
}

#[test]
fn a_read_filling_the_budget_exactly_passes() {
    let budget = Budget::new(Limits {
        max_source_bytes: 5,
    });

    assert_eq!(
        budget.read_to_end(&mut &b"12345"[..]).expect("read"),
        Some(b"12345".to_vec())
    );
    assert_eq!(budget.remaining(), 0);
    assert!(!budget.exhausted());
}

#[test]
fn an_oversized_read_holds_one_byte_past_the_bound() {
    let budget = Budget::new(Limits {
        max_source_bytes: 4,
    });
    let source = vec![0u8; 1 << 16];

    assert_eq!(
        budget.read_to_end(&mut source.as_slice()).expect("read"),
        None
    );
    assert!(budget.exhausted());
}
