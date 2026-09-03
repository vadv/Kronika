#![allow(dead_code)]

#[derive(Clone, Copy)]
enum Shared {
    One,
    Two,
}

type Alias = Shared;

fn identity(value: Shared) -> Alias {
    match value {
        Shared::One => Alias::One,
        Shared::Two => Alias::Two,
    }
}

fn consume<F: Fn() -> bool>(cancelled: &F) -> bool {
    cancelled()
}

fn forwarding(cancelled: &dyn Fn() -> bool) -> bool {
    consume(&|| cancelled())
}

fn discarded_error() {
    std::fs::metadata(".").ok();
}

fn main() {}
