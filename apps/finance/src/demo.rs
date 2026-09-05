use crate::model::{Id, Ledger};
use crate::runtime::{ImportState, Runtime, Start};
use makepad_widgets::{Actions, Cx};

#[derive(Default)]
pub(crate) struct Backend;

impl Runtime for Backend {
    fn start(&mut self) -> Start {
        let today = crate::runtime::demo_today();
        Start {
            today,
            ledger: crate::seed::generate(crate::seed::DEFAULT_YEARS, today),
            status: "Demo household loaded".to_string(),
        }
    }

    fn has_import(&self) -> bool {
        false
    }

    fn pick_statement(&mut self, _cx: &mut Cx) {}

    fn prepare_from_actions(
        &mut self,
        _actions: &Actions,
        _ledger: &Ledger,
        _account_filter: Option<Id>,
    ) -> Option<Result<ImportState, String>> {
        None
    }

    fn commit_import(&mut self, _state: ImportState) -> Result<(Ledger, String), String> {
        Err("statement import is unavailable in the demo".to_string())
    }
}
