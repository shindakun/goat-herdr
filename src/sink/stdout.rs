//! Prints the alert. Used for dry runs and as the default when no sink is
//! configured; output lands in `herdr plugin log list`.

use crate::alert::Alert;
use crate::sink::Sink;

pub struct Stdout;

impl Sink for Stdout {
    fn name(&self) -> &str {
        "stdout"
    }

    fn send(&self, alert: &Alert) -> Result<(), String> {
        println!("{}", alert.headline());
        println!("pane {}", alert.pane_id);
        if let Some(tail) = &alert.tail {
            println!("---\n{tail}");
        }
        Ok(())
    }
}
