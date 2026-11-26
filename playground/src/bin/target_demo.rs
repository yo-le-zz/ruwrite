//! Programme cible : il n'a rien prévu pour être modifié.
//!
//! Construit en debug (opt-level 0, informations DWARF), il affiche une ligne
//! `clé=valeur` par élément, ce qui permet de vérifier chaque réécriture.

use std::hint::black_box;

static LIMIT: i32 = 10;
static RATIO: f64 = 1.5;
static BANNER: [u8; 5] = *b"hello";
static SMALL: u8 = 7;

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
fn is_admin() -> bool {
    false
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
    println!("limit={}", black_box(&LIMIT));
    println!("ratio={}", black_box(&RATIO));
    println!("banner={}", String::from_utf8_lossy(black_box(&BANNER)));
    println!("small={}", black_box(&SMALL));
    println!("compute={}", compute());
    println!("small_local={}", small_local());
    println!("admin={}", is_admin());
    println!("scale={}", scale());
    say_hi();
    println!("redirected={}", redirected());
    // Garde `replacement` dans le binaire.
    black_box(replacement as fn() -> i32);
}
