#![allow(dead_code)]

// Immutable repository reference:
// <https://github.com/example/project/blob/8c1f6e2a6d33c1b1a2f9e0e1d3b8a4c7d6e5f4a3/src/lib.rs>

#[derive(Clone, Copy)]
enum Shared {
    One,
    Two,
}

fn nonidentity(value: Shared) -> Shared {
    match value {
        Shared::One => Shared::Two,
        Shared::Two => Shared::One,
    }
}

fn guarded(value: Shared, keep: bool) -> Shared {
    match value {
        Shared::One if keep => Shared::One,
        Shared::One => Shared::One,
        Shared::Two => Shared::Two,
    }
}

enum Payload {
    One(u8),
    Two,
}

fn payload(value: Payload) -> Payload {
    match value {
        Payload::One(inner) => Payload::One(inner),
        Payload::Two => Payload::Two,
    }
}

fn consume(cancelled: &(impl Fn() -> bool + ?Sized)) -> bool {
    cancelled()
}

fn direct(cancelled: &dyn Fn() -> bool) -> bool {
    consume(cancelled)
}

fn extra_work(cancelled: &dyn Fn() -> bool) -> bool {
    consume(&|| {
        let cancelled = cancelled();
        cancelled && std::hint::black_box(true)
    })
}

fn bound_macro_side_effect(counter: &mut u8) {
    let value = {
        *counter += 1;
        *counter
    };
    debug_assert_eq!(value, 1);
}

fn main() {}
