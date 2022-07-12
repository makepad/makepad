use {
    crate::{
        makepad_editor_core::{
            delta::Delta,
            text::Text
        },
        makepad_live_id::LiveIdMap,
        makepad_collab_protocol::{
            DirectoryEntry,
            TextFileId,
            FileNodeData,
            FileTreeData,
            CollabError,
            CollabNotification,
            CollabRequest,
            CollabResponse,
        },
    },
    std::{ 
        cmp::Ordering,
        collections::{HashMap, VecDeque},
        fmt,
        fs,
        mem,
        path::{Path, PathBuf},
        sync::{Arc, Mutex, RwLock},
    },
};

/// A collab server.
/// 
/// The collab server is designed to be transport agnostic. That is, it does not make any
/// assumptions about whether it is running over Tcp, WebSockets, etc. The idea is that an embedder
/// can take the server and easily implement its own transport layer on top of it.
pub struct CollabServer {
    // The id for the next connection
    next_connection_id: usize,
    // State that is shared between every connection
    shared: Arc<RwLock<Shared>>,
}

impl CollabServer {
    /// Creates a new collab server rooted at the given path.
    pub fn new<P: Into<PathBuf>>(path: P) -> CollabServer {
        CollabServer {
            next_connection_id: 0,
            shared: Arc::new(RwLock::new(Shared {
                path: path.into(),
                files: LiveIdMap::new(),
                file_ids_by_path: HashMap::new(),
            })),
        }
    }
    
    /// Creates a new connection to this collab server.
    /// 
    /// The given `notification_sender` is called whenever the server wants to send a notification
    /// for this connection. The embedder is responsible for sending the notification.
    pub fn connect(&mut self, notification_sender: Box<dyn NotificationSender>) -> CollabConnection {
        let connection_id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        CollabConnection {
            connection_id,
            shared: self.shared.clone(),
            notification_sender,
        }
    }
}

/// A connection to a collab server.
pub struct CollabConnection {
    // The id for this connection.
    connection_id: ConnectionId,
    // State is shared between every connection.
    shared: Arc<RwLock<Shared>>,
    // Used to send notifications for this connection.
    notification_sender: Box<dyn NotificationSender>,
}

impl CollabConnection {
    /// Handles the given `request` for this connection, and returns the corresponding response.
    /// 
    /// The embedder is responsible for receiving requests, calling this method to handle them, and
    /// sending back the response.
    pub fn handle_request(&self, request: CollabRequest) -> CollabResponse {
        match request {
            CollabRequest::LoadFileTree {with_data} => CollabResponse::LoadFileTree(self.load_file_tree(with_data)),
            CollabRequest::OpenFile(path) => {
                let mut base_path = self.shared.read().unwrap().path.clone();
                base_path.push(path);
                CollabResponse::OpenFile(self.open_file(base_path))
            }
            CollabRequest::ApplyDelta(text_file_id, revision, delta) => {
                CollabResponse::ApplyDelta(self.apply_delta(text_file_id, revision, delta))
            }
            CollabRequest::CloseFile(path) => CollabResponse::CloseFile(self.close_file(path)),
        }
    }
    
    // Handles a `LoadFileTree` request.
    fn load_file_tree(&self, with_data: bool) -> Result<FileTreeData, CollabError> {
        // A recursive helper function for traversing the entries of a directory and creating the
        // data structures that describe them.
        fn get_directory_entries(path: &Path, with_data: bool) -> Result<Vec<DirectoryEntry>, CollabError> {
            let mut entries = Vec::new();
            for entry in fs::read_dir(path).map_err( | error | CollabError::Unknown(error.to_string()))? {
                // We can't get the entry for some unknown reason. Raise an error.
                let entry = entry.map_err( | error | CollabError::Unknown(error.to_string()))?;
                // Get the path for the entry.
                let entry_path = entry.path();
                // Get the file name for the entry.
                let name = entry.file_name();
                if let Ok(name_string) = name.into_string() {
                    if entry_path.is_dir() && name_string == "target"
                        || name_string.starts_with('.') {
                        // Skip over directories called "target". This is sort of a hack. The reason
                        // it's here is that the "target" directory for Rust projects is huge, and
                        // our current implementation of the file tree widget is not yet fast enough
                        // to display vast numbers of nodes. We paper over this by pretending the
                        // "target" directory does not exist.
                        continue;
                    }
                }
                else {
                    // Skip over entries with a non UTF-8 file name.
                    continue;
                }
                // Create a `DirectoryEntry` for this entry and add it to the list of entries.
                entries.push(DirectoryEntry {
                    name: entry.file_name(),
                    node: if entry_path.is_dir() {
                        // If this entry is a subdirectory, recursively create `DirectoryEntry`'s
                        // for its entries as well.
                        FileNodeData::Directory {
                            entries: get_directory_entries(&entry_path, with_data) ?,
                        }
                    } else if entry_path.is_file() {
                        if with_data {
                            let bytes: Vec<u8> = fs::read(&entry_path).map_err(
                                | error | CollabError::Unknown(error.to_string())
                            ) ?;
                            FileNodeData::File {data: Some(bytes)}
                        }
                        else {
                            FileNodeData::File {data: None}
                        }
                    }
                    else {
                        // If this entry is neither a directory or a file, skip it. This ignores
                        // things such as symlinks, for which we are not yet sure how we want to
                        // handle them.
                        continue
                    },
                });
            }
            
            /// Sort all the entries by name, directories first, and files second.
            entries.sort_by( | entry_0, entry_1 | {
                match &entry_0.node{
                    FileNodeData::Directory{..}=>match &entry_1.node{
                        FileNodeData::Directory{..}=>entry_0.name.cmp(&entry_1.name),
                        FileNodeData::File{..}=>Ordering::Less
                    }
                    FileNodeData::File{..}=>match &entry_1.node{
                        FileNodeData::Directory{..}=>Ordering::Greater,
                        FileNodeData::File{..}=>entry_0.name.cmp(&entry_1.name)
                    }
                }
            });
            Ok(entries)
        }
        
        let path = self.shared.read().unwrap().path.clone();

        let root = FileNodeData::Directory {
            entries: get_directory_entries(&path, with_data) ?,
        };
        Ok(FileTreeData {path:"".into(), root})
    }
    
    // Handles an `OpenFile` request.
    fn open_file(&self, path: PathBuf) -> Result<(TextFileId, usize, Text), CollabError> {
        let mut shared_guard = self.shared.write().unwrap();
        match shared_guard.file_ids_by_path.get(&path) {
            Some(&file_id) => {
                let mut file_guard = shared_guard.files[file_id].lock().unwrap();
                
                let their_revision = file_guard.our_revision;
                let text = file_guard.text.clone();
                if file_guard
                    .participants_by_connection_id
                    .contains_key(&self.connection_id)
                {
                    return Err(CollabError::AlreadyAParticipant);
                }
                file_guard.participants_by_connection_id.insert(
                    self.connection_id,
                    Participant {
                        their_revision,
                        notification_sender: self.notification_sender.clone(),
                    },
                );
                
                drop(file_guard);
                
                drop(shared_guard);
                
                Ok((file_id, their_revision, text))
            }
            None => {
                let bytes = fs::read(&path).map_err(
                    | error | CollabError::Unknown(error.to_string())
                ) ?;
                let text: Text = Text::from_lines(String::from_utf8_lossy(&bytes)
                    .lines()
                    .map( | line | line.chars().collect::<Vec<_ >> ())
                    .collect::<Vec<_ >>());
                
                let mut participants_by_connection_id = HashMap::new();
                participants_by_connection_id.insert(
                    self.connection_id,
                    Participant {
                        their_revision: 0,
                        notification_sender: self.notification_sender.clone(),
                    },
                );
                let file = Mutex::new(File {
                    path: path.clone(),
                    our_revision: 0,
                    text: text.clone(),
                    outstanding_deltas: VecDeque::new(),
                    participants_by_connection_id,
                });
                
                let file_id = shared_guard.files.insert_unique(file);
                shared_guard.file_ids_by_path.insert(path, file_id);
                
                drop(shared_guard);
                
                Ok((file_id, 0, text))
            }
        }
    }
    
    // Handles an `ApplyDelta` request.
    fn apply_delta(
        &self,
        file_id: TextFileId,
        their_revision: usize,
        delta: Delta,
    ) -> Result<TextFileId, CollabError> {
        let shared_guard = self.shared.read().unwrap();
        
        let mut file_guard = shared_guard.files[file_id].lock().unwrap();
        
        let unseen_delta_count = file_guard.our_revision - their_revision;
        let seen_delta_count = file_guard.outstanding_deltas.len() - unseen_delta_count;
        let mut delta = delta;
        for unseen_delta in file_guard.outstanding_deltas.iter().skip(seen_delta_count) {
            delta = unseen_delta.clone().transform(delta).1;
        }
        
        file_guard.our_revision += 1;
        file_guard.text.apply_delta(delta.clone());
        file_guard.outstanding_deltas.push_back(delta.clone());
        
        let participant = file_guard
            .participants_by_connection_id
            .get_mut(&self.connection_id)
            .unwrap();
        participant.their_revision = their_revision;
        
        let settled_revision = file_guard
            .participants_by_connection_id
            .values()
            .map( | participant | participant.their_revision)
            .min()
            .unwrap();
        let unsettled_delta_count = file_guard.our_revision - settled_revision;
        let settled_delta_count = file_guard.outstanding_deltas.len() - unsettled_delta_count;
        file_guard.outstanding_deltas.drain(..settled_delta_count);
        
        file_guard.notify_other_participants(
            self.connection_id,
            CollabNotification::DeltaWasApplied(file_id, delta),
        );
        
        drop(file_guard);
        
        drop(shared_guard);
        
        Ok(file_id)
    }
    
    // Handles a `CloseFile` request.
    fn close_file(&self, file_id: TextFileId) -> Result<TextFileId, CollabError> {
        let mut shared_guard = self.shared.write().unwrap();
        
        let mut file_guard = shared_guard.files[file_id].lock()
            .map_err( | _ | CollabError::NotAParticipant) ?;
        
        if !file_guard
            .participants_by_connection_id
            .contains_key(&self.connection_id)
        {
            return Err(CollabError::NotAParticipant);
        }
        file_guard
            .participants_by_connection_id
            .remove(&self.connection_id);
        let is_empty = file_guard.participants_by_connection_id.is_empty();
        
        if is_empty {
            let path = mem::replace(&mut file_guard.path, PathBuf::new());
            drop(file_guard);
            shared_guard.file_ids_by_path.remove(&path);
            shared_guard.files.remove(&file_id);
        } else {
            drop(file_guard);
        }
        
        drop(shared_guard);
        
        Ok(file_id)
    }
}

pub trait NotificationSender: Send {
    fn box_clone(&self) -> Box<dyn NotificationSender>;
    
    fn send_notification(&self, notification: CollabNotification);
}

impl<F: Clone + Fn(CollabNotification) + Send + 'static> NotificationSender for F {
    fn box_clone(&self) -> Box<dyn NotificationSender> {
        Box::new(self.clone())
    }
    
    fn send_notification(&self, notification: CollabNotification) {
        self (notification)
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

// State that is shared between every connection.
#[derive(Debug)]
struct Shared {
    path: PathBuf,
    files: LiveIdMap<TextFileId, Mutex<File >>,
    file_ids_by_path: HashMap<PathBuf, TextFileId>,
}

/// An identifier for a connection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionId(usize);

#[derive(Debug)]
struct File {
    path: PathBuf,
    our_revision: usize,
    text: Text,
    outstanding_deltas: VecDeque<Delta>,
    participants_by_connection_id: HashMap<ConnectionId, Participant>,
}

impl File {
    fn notify_other_participants(&self, connection_id: ConnectionId, notification: CollabNotification) {
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
