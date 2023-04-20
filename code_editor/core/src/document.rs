use {
    crate::{Session, Text, WeakPtrEq},
    makepad_futures::task::Spawn,
    std::{
        cell::RefCell,
        collections::HashSet,
        future::Future,
        rc::{Rc, Weak},
    },
};

#[derive(Debug)]
pub struct Document {
    sessions: HashSet<WeakPtrEq<RefCell<Session>>>,
    text: Text,
}

impl Document {
    pub fn new(
        spawner: &mut impl Spawn,
        load: impl Future<Output = Text> + 'static,
    ) -> Rc<RefCell<Self>> {
        let document = Rc::new(RefCell::new(Self {
            sessions: HashSet::new(),
            text: ["Loading...".into()].into(),
        }));
        spawner
            .spawn({
                let document = document.clone();
                async move {
                    let text = load.await;
                    let mut document = document.borrow_mut();
                    document.text = text;
                }
            })
            .unwrap();
        document
    }

    pub fn sessions(&self) -> &HashSet<WeakPtrEq<RefCell<Session>>> {
        &self.sessions
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn insert_session(&mut self, session: Weak<RefCell<Session>>) {
        self.sessions.insert(WeakPtrEq(session));
    }

    pub fn remove_session(&mut self, session: Weak<RefCell<Session>>) {
        self.sessions.remove(&WeakPtrEq(session));
    }
}
