//! Programme qui tourne en boucle : sert à tester la modification à chaud.
//!
//! Affiche `value=<n>` toutes les 50 ms ; `value()` est la fonction que les
//! tests réécrivent pendant l'exécution.

use std::io::Write;
use std::thread::sleep;
use std::time::Duration;

#[inline(never)]
fn value() -> i32 {
    1
}

fn main() {
    let stdout = std::io::stdout();
    loop {
        {
            let mut out = stdout.lock();
            if writeln!(out, "value={}", value())
                .and_then(|_| out.flush())
                .is_err()
            {
                return; // lecteur fermé : on s'arrête
            }
        }
        sleep(Duration::from_millis(50));
    }
}
