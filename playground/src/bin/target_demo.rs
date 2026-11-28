//! Programme cible : il n'a rien prévu pour être modifié.
//!
//! Construit en debug (opt-level 0, informations DWARF), il affiche une ligne
//! `clé=valeur` par élément, ce qui permet de vérifier chaque réécriture.

use ruwrite::protect;
use std::hint::black_box;

static LIMIT: i32 = 10;
static RATIO: f64 = 1.5;
static BANNER: [u8; 5] = *b"hello";
static SMALL: u8 = 7;

#[protect(protected)]
static SECRET: i32 = 1234;

#[protect(immutable)]
#[inline(never)]
fn license_check() -> bool {
    false
}

mod secure {
    use ruwrite::protect;

    #[protect(immutable)]
    #[inline(never)]
    pub fn token() -> i32 {
        99
    }
}

// Caché : ni le nom ni la liste des fonctions de ruwrite ne la montrent.
#[protect(hide)]
#[inline(never)]
fn hidden_formula() -> i32 {
    7
}

#[inline(never)]
fn compute() -> i32 {
    let a: i32 = 3;
    let b: i32 = 4;
    // Lecture via référence : la valeur est relue depuis l'emplacement de pile.
    *black_box(&a) + *black_box(&b)
}

#[inline(never)]
fn small_local() -> i64 {
    let big: i64 = 1000;
    let flag: bool = false;
    if *black_box(&flag) {
        -1
    } else {
        *black_box(&big)
    }
}

#[inline(never)]
fn counter() -> i32 {
    let mut c: i32 = 1;
    let fixed: i32 = 5;
    *black_box(&mut c) + *black_box(&fixed)
}

#[inline(never)]
fn is_admin() -> bool {
    false
}

/// `a`'s initial value is overwritten by `x` before the comparison, which
/// keeps `a` in its stack slot and the `a < 5` test compiled as a direct
/// `cmp [slot], 5` (no constant folding, no register in between): exactly
/// the shape cascade mode looks for.
#[inline(never)]
#[allow(unused_assignments)]
fn cascade_check(x: i32) -> i32 {
    let mut a: i32 = 3;
    a = x;
    if a < 5 {
        1
    } else {
        2
    }
}

#[inline(never)]
fn scale() -> f64 {
    let factor: f64 = 1.0;
    *black_box(&factor) * 1.0
}

#[inline(never)]
fn say_hi() {
    println!("hi=called");
}

#[inline(never)]
fn original_target() -> i32 {
    1
}

#[inline(never)]
fn replacement() -> i32 {
    2
}

#[inline(never)]
fn redirected() -> i32 {
    original_target()
}

fn main() {
    println!("license={}", license_check());
    println!("secret={}", black_box(&SECRET));
    println!("token={}", secure::token());
    println!("limit={}", black_box(&LIMIT));
    println!("ratio={}", black_box(&RATIO));
    println!("banner={}", String::from_utf8_lossy(black_box(&BANNER)));
    println!("small={}", black_box(&SMALL));
    println!("compute={}", compute());
    println!("small_local={}", small_local());
    println!("counter={}", counter());
    println!("admin={}", is_admin());
    println!("scale={}", scale());
    say_hi();
    println!("redirected={}", redirected());
    println!("hidden={}", hidden_formula());
    println!("cascade={}", cascade_check(black_box(-100)));
    // Garde `replacement` dans le binaire.
    black_box(replacement as fn() -> i32);
}
