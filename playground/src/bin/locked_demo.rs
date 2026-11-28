//! Programme entièrement protégé : ruwrite refuse toute modification.

ruwrite::protect_all!();

static VALUE: i32 = 5;

#[inline(never)]
fn answer() -> i32 {
    42
}

fn main() {
    println!("value={}", std::hint::black_box(&VALUE));
    println!("answer={}", answer());
}
