use {
    crate::{
        delta::Delta,
        protocol::{DirectoryEntry, Error, FileNode, Notification, Request, Response},
        text::Text,
    },
    std::{
        collections::{HashMap, VecDeque},
        fmt, fs,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex, RwLock,
        },
    },
};

#[derive(Clone)]
pub struct Server {
    shared: Arc<Shared>,
}

impl Server {
    pub fn new<P: Into<PathBuf>>(path: P) -> Server {
        Server {
            shared: Arc::new(Shared {
                next_connection_id: AtomicUsize::new(0),
                path: path.into(),
                documents_by_path: RwLock::new(HashMap::new()),
            }),
        }
    }

    pub fn connect(&self, notification_sender: Box<dyn NotificationSender>) -> Connection {
        Connection {
            connection_id: ConnectionId(
                self.shared
                    .next_connection_id
                    .fetch_add(1, Ordering::SeqCst),
            ),
            shared: self.shared.clone(),
            notification_sender,
        }
    }
}

pub struct Connection {
    connection_id: ConnectionId,
    shared: Arc<Shared>,
    notification_sender: Box<dyn NotificationSender>,
}

impl Connection {
    pub fn handle_request(&self, request: Request) -> Response {
        match request {
            Request::GetFileTree() => Response::GetFileTree(self.get_file_tree()),
            Request::OpenFile(path) => Response::OpenFile(self.open_file(path)),
            Request::ApplyDelta(path, revision, delta) => {
                Response::ApplyDelta(self.apply_delta(path, revision, delta))
            }
            Request::CloseFile(path) => Response::CloseFile(self.close_file(path)),
        }
    }

    pub fn get_file_tree(&self) -> Result<FileNode, Error> {
        fn get_directory_entries(path: &Path) -> Result<Vec<DirectoryEntry>, Error> {
            let mut entries = Vec::new();
            for entry in fs::read_dir(path).map_err(|error| Error::Unknown(error.to_string()))? {
                let entry = entry.map_err(|error| Error::Unknown(error.to_string()))?;
                let entry_path = entry.path();
                entries.push(DirectoryEntry {
                    name: entry.file_name(),
                    node: if entry_path.is_dir() {
                        FileNode::Directory {
                            entries: get_directory_entries(&entry_path)?,
                        }
                    } else {
                        FileNode::File
                    },
                });
            }
            Ok(entries)
        }

        Ok(FileNode::Directory {
            entries: get_directory_entries(&self.shared.path)?,
        })
    }

    pub fn open_file(&self, path: PathBuf) -> Result<(usize, Text), Error> {
        let mut documents_by_path_guard = self.shared.documents_by_path.write().unwrap();
        match documents_by_path_guard.get(&path) {
            Some(document) => {
                let mut document_guard = document.lock().unwrap();

                let their_revision = document_guard.our_revision;
                document_guard.participants_by_connection_id.insert(
                    self.connection_id,
                    Participant {
                        their_revision,
                        notification_sender: self.notification_sender.clone(),
                    },
                );

                let text = document_guard.text.clone();
                drop(document_guard);

                drop(documents_by_path_guard);

                Ok((their_revision, text))
            }
            None => {
                let bytes = fs::read(&path).map_err(|error| Error::Unknown(error.to_string()))?;
                let text: Text = String::from_utf8_lossy(&bytes)
                    .lines()
                    .map(|line| line.chars().collect::<Vec<_>>())
                    .collect::<Vec<_>>()
                    .into();

                let mut participants_by_connection_id = HashMap::new();
                participants_by_connection_id.insert(
                    self.connection_id,
                    Participant {
                        their_revision: 0,
                        notification_sender: self.notification_sender.clone(),
                    },
                );

                documents_by_path_guard.insert(
                    path,
                    Mutex::new(Document {
                        our_revision: 0,
                        text: text.clone(),
                        outstanding_deltas: VecDeque::new(),
                        participants_by_connection_id,
                    }),
                );

                drop(documents_by_path_guard);

                Ok((0, text))
            }
        }
    }

    fn apply_delta(&self, path: PathBuf, their_revision: usize, delta: Delta) -> Result<(), Error> {
        let documents_by_path_guard = self.shared.documents_by_path.read().unwrap();

        let document = documents_by_path_guard.get(&path).unwrap();
        let mut document_guard = document.lock().unwrap();

        let unseen_delta_count = document_guard.our_revision - their_revision;
        let seen_delta_count = document_guard.outstanding_deltas.len() - unseen_delta_count;
        let mut delta = delta;
        for unseen_delta in document_guard
            .outstanding_deltas
            .iter()
            .skip(seen_delta_count)
        {
            delta = unseen_delta.clone().transform(delta).1;
        }

        document_guard.our_revision += 1;
        document_guard.text.apply_delta(delta.clone());
        document_guard.outstanding_deltas.push_back(delta.clone());

        let participant = document_guard
            .participants_by_connection_id
            .get_mut(&self.connection_id)
            .unwrap();
        participant.their_revision = their_revision;

        let settled_revision = document_guard
            .participants_by_connection_id
            .values()
            .map(|participant| participant.their_revision)
            .min()
            .unwrap();
        let unsettled_delta_count = document_guard.our_revision - settled_revision;
        let settled_delta_count = document_guard.outstanding_deltas.len() - unsettled_delta_count;
        document_guard.outstanding_deltas.drain(..settled_delta_count);

        document_guard.notify_other_participants(
            self.connection_id,
            Notification::DeltaWasApplied(path, delta),
        );

        drop(document_guard);

        drop(documents_by_path_guard);

        Ok(())
    }

    fn close_file(&self, path: PathBuf) -> Result<(), Error> {
        let mut documents_by_path_guard = self.shared.documents_by_path.write().unwrap();

        let document = documents_by_path_guard.get(&path).unwrap();
        let mut document_guard = document.lock().unwrap();

        document_guard.participants_by_connection_id.remove(&self.connection_id);

        let is_empty = document_guard.participants_by_connection_id.is_empty();
        drop(document_guard);

        if is_empty {
            documents_by_path_guard.remove(&path);
        }

        drop(documents_by_path_guard);

        Ok(())
    }
}

pub trait NotificationSender: Send {
    fn box_clone(&self) -> Box<dyn NotificationSender>;

    fn send_notification(&self, notification: Notification);
}

impl<F: Clone + Fn(Notification) + Send + 'static> NotificationSender for F {
    fn box_clone(&self) -> Box<dyn NotificationSender> {
        Box::new(self.clone())
    }

    fn send_notification(&self, notification: Notification) {
        self(notification)
    }
}

impl Clone for Box<dyn NotificationSender> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}

impl fmt::Debug for dyn NotificationSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NotificationSender")
    }
}

#[derive(Debug)]
struct Shared {
    path: PathBuf,
    next_connection_id: AtomicUsize,
    documents_by_path: RwLock<HashMap<PathBuf, Mutex<Document>>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionId(usize);

#[derive(Debug)]
struct Document {
    our_revision: usize,
    text: Text,
    outstanding_deltas: VecDeque<Delta>,
    participants_by_connection_id: HashMap<ConnectionId, Participant>,
}

impl Document {
    fn notify_other_participants(&self, connection_id: ConnectionId, notification: Notification) {
        for (other_connection_id, other_participant) in &self.participants_by_connection_id {
            if *other_connection_id == connection_id {
                continue;
            }
            other_participant
                .notification_sender
                .send_notification(notification.clone())
        }
    }
}

#[derive(Debug)]
struct Participant {
    their_revision: usize,
    notification_sender: Box<dyn NotificationSender>,
}
