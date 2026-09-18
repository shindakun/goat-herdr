//! Prints the alert. Used for dry runs and as the default when no sink is
//! configured; output lands in `herdr plugin log list`.

use crate::alert::Alert;
use crate::sink::{Delivery, Sink};

pub struct Stdout;

impl Sink for Stdout {
    fn name(&self) -> &str {
        "stdout"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        println!("{}", super::plain_text(alert));
        Ok(Delivery::Sent)
    }
}
