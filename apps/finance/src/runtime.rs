//! Build-specific storage and import capabilities behind one UI-facing API.

use crate::date::{self, Day};
use crate::model::{Id, Ledger};
use makepad_widgets::{Actions, Cx};

pub(crate) struct Start {
    pub today: Day,
    pub ledger: Ledger,
    pub status: String,
}

pub(crate) struct ImportState {
    pub path: String,
    pub plan: crate::import::Plan,
    pub ask_date_order: bool,
}

pub(crate) fn demo_today() -> Day {
    date::from_ymd(2026, 8, 28)
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "demo")))]
#[path = "native.rs"]
mod imp;
#[cfg(any(target_arch = "wasm32", feature = "demo"))]
#[path = "demo.rs"]
mod imp;

pub(crate) use imp::Backend;

pub(crate) trait Runtime {
    fn start(&mut self) -> Start;
    fn has_import(&self) -> bool;
    fn pick_statement(&mut self, cx: &mut Cx);
    fn prepare_from_actions(
        &mut self,
        actions: &Actions,
        ledger: &Ledger,
        account_filter: Option<Id>,
    ) -> Option<Result<ImportState, String>>;
    fn commit_import(&mut self, state: ImportState) -> Result<(Ledger, String), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_clock_is_pinned_late_in_august() {
        assert_eq!(date::to_ymd(demo_today()), (2026, 8, 28));
        assert_eq!(date::month_key(demo_today()), date::month_key(date::from_ymd(2026, 8, 1)));
    }
}
