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
            unix_str::UnixString,
        },
    },
    std::{ 
        cmp::Ordering,
        collections::{HashMap, VecDeque},
        fmt,
        fs,
        mem,
        io::prelude::*,
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
    
    /// Creates a new connection to this collab server, and returns a handle for the connection.
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
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        match request {
            CollabRequest::LoadFileTree {with_data} => CollabResponse::LoadFileTree(self.load_file_tree(with_data)),
            CollabRequest::OpenFile(path) => {
                let path = PathBuf::from(OsString::from_vec(path.into_unix_string().into_vec()));
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
        use std::os::unix::ffi::OsStringExt;
        
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
                    name: UnixString::from_vec(entry.file_name().into_vec()),
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
            
            // Sort all the entries by name, directories first, and files second.
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
    fn open_file(&self, path: PathBuf) -> Result<(TextFileId, u32, Text), CollabError> {
        // We need to update the list of files in the shared state, so lock it for writing. This is
        // necessary so other clients cannot close the file while we are still in the process of
        // opening it.
        let mut shared_guard = self.shared.write().unwrap();

        match shared_guard.file_ids_by_path.get(&path) {
            Some(&file_id) => {
                // The file was already open, so we just need to add the client as a participant to
                // it.

                // We need to obtain a snapshot of the state of the file to send back to the client
                // (it's revision + contents). Lock the file for access so other clients cannot
                // apply further deltas to the file while we are making the snapshot.
                let mut file_guard = shared_guard.files[file_id].lock().unwrap();
                
                // Their (the client) initial revision will be the same as ours (the server).
                let their_revision = file_guard.our_revision;
                // Get a copy of the contents of the file.
                let text = file_guard.text.clone();
                if file_guard
                    .participants_by_connection_id
                    .contains_key(&self.connection_id)
                {
                    // The client is already a participant for this file. Raise an error.
                    return Err(CollabError::AlreadyAParticipant);
                }
                // Add the client as a participant.
                file_guard.participants_by_connection_id.insert(
                    self.connection_id,
                    Participant {
                        their_revision,
                        notification_sender: self.notification_sender.clone(),
                    },
                );
                
                // It's now safe to drop our locks.
                drop(file_guard);
                
                drop(shared_guard);
                
                Ok((file_id, their_revision as u32, text))
            }
            None => {
                // The file was not yet opened, so we need to open it, and then add the client as
                // the only participant to it. In this case we don't need to lock the file, since
                // it doesn't yet exist, so nobody else can have a reference to it.

                // Get the contents of the file from disk. If this fails for some unknown reason,
                // raise an error.
                let bytes = fs::read(&path).map_err(
                    | error | CollabError::Unknown(error.to_string())
                ) ?;
                // Converts the file contents to a `Text`. This is necessarily a lossy conversion
                // because `Text` assumes everything is UTF-8 encoded, and this isn't always the
                // case for files on disk (is this a problem?)
                let text: Text = Text::from_lines(String::from_utf8_lossy(&bytes)
                    .lines()
                    .map( | line | line.chars().collect::<Vec<_ >> ())
                    .collect::<Vec<_ >>());
                
                // Create the list of participants for this file and add the file to it.
                let mut participants_by_connection_id = HashMap::new();
                participants_by_connection_id.insert(
                    self.connection_id,
                    Participant {
                        their_revision: 0,
                        notification_sender: self.notification_sender.clone(),
                    },
                );

                // Create the file
                let file = Mutex::new(File {
                    path: path.clone(),
                    our_revision: 0,
                    text: text.clone(),
                    outstanding_deltas: VecDeque::new(),
                    participants_by_connection_id,
                });
                
                // Insert the file in the shared list of files.
                let file_id = shared_guard.files.insert_unique(file);
                shared_guard.file_ids_by_path.insert(path, file_id);
                
                // It's now safe to drop our locks.
                drop(shared_guard);
                
                Ok((file_id, 0, text))
            }
        }
    }
    
    // Handles an `ApplyDelta` request.
    fn apply_delta(
        &self,
        file_id: TextFileId,
        their_revision: u32,
        delta: Delta,
    ) -> Result<TextFileId, CollabError> {
        // We need only need to get the list of files in the shared state, so lock it for reading.
        // This is necessary so other clients cannot close the file while we are still in the
        // process of applying the delta to it.
        let shared_guard = self.shared.read().unwrap();
        
        // We're going to modify the state of the file. Lock the file for access so other clients
        // cannot make concurrent modifications to the state while we are still working on it.
        let mut file_guard = shared_guard.files[file_id].lock().unwrap();
        
        // The number of deltas that has been seen by the server but not the client.
        let unseen_delta_count = file_guard.our_revision - their_revision ;
        // The number of deltas that has been seen by both the server and the client.
        let seen_delta_count = file_guard.outstanding_deltas.len() as u32 - unseen_delta_count;

        // Transform the delta against each delta that has been seen by the server but not by the
        // client to obtain a delta that can be applied to the newest revision of the file.
        let mut delta = delta;
        for unseen_delta in file_guard.outstanding_deltas.iter().skip(seen_delta_count as usize) {
            delta = unseen_delta.clone().transform(delta).1;
        }
        
        // Apply the delta to the file, increment its revision by one, and then store the delta
        // in the list of deltas that has been seen by the server but not *every* client.
        file_guard.our_revision += 1;
        file_guard.text.apply_delta(delta.clone());
        file_guard.outstanding_deltas.push_back(delta.clone());
        
        if let Ok(mut file) = fs::File::create(&file_guard.path){
            if let Err(_) = file.write_all(format!("{}", file_guard.text).as_bytes()){
                eprintln!("Error writing file {:?}", file_guard.path)
            }
        }
        else{
            eprintln!("Error opening file {:?}", file_guard.path)
        }
        
        // Update the last revision that has been seen by the client.
        let participant = file_guard
            .participants_by_connection_id
            .get_mut(&self.connection_id)
            .unwrap();
        participant.their_revision = their_revision;
        
        // Compute the oldest revision that has been seen by both the server and *every* client.
        let settled_revision = file_guard
            .participants_by_connection_id
            .values()
            .map( | participant | participant.their_revision)
            .min()
            .unwrap();
        // The number of deltas that has been seen by the server, but not *every* client.
        let unsettled_delta_count = file_guard.our_revision - settled_revision;
        // The number of deltas that has been seen by both the server and *every* client.
        let settled_delta_count = file_guard.outstanding_deltas.len() as u32 - unsettled_delta_count;
        // Remove any deltas that have been seen by both the server and *every* client from the list
        // of deltas that have been seen by the server but not *every* client.
        file_guard.outstanding_deltas.drain(..(settled_delta_count as usize));
        
        // Notify the other participants that a delta has been applied to this file.
        file_guard.notify_other_participants(
            self.connection_id,
            CollabNotification::DeltaWasApplied(file_id, delta),
        );
        
        // It's now safe to drop our locks.
        drop(file_guard);
        
        drop(shared_guard);
        
        Ok(file_id)
    }
    
    // Handles a `CloseFile` request.
    fn close_file(&self, file_id: TextFileId) -> Result<TextFileId, CollabError> {
        // We need to update the list of files in the shared state, so lock it for writing. This is
        // necessary so other clients cannot reopen file while we are still in the process of
        // closing it.
        let mut shared_guard = self.shared.write().unwrap();
        
        // We need to modify the list of participants for the file, so lock the file for access so
        // other clients cannot further modify it while we are still in the process of doing so.
        let mut file_guard = shared_guard.files[file_id].lock()
            .map_err(| _ | {
                // The client is already a participant for this file (because it doesn't even
                // exist). Raise an error.
                CollabError::NotAParticipant
            })?;
        
        if !file_guard
            .participants_by_connection_id
            .contains_key(&self.connection_id)
        {
            // The client is not a participant for this file. Raise an error.
            return Err(CollabError::NotAParticipant);
        }

        // Remove the client from the list of participants for this file.
        file_guard
            .participants_by_connection_id
            .remove(&self.connection_id);
        let is_empty = file_guard.participants_by_connection_id.is_empty();
        
        if is_empty {
            // If the list of participants for the file is now empty, it's time to close the file
            // and remove it from the shared list of files.
            let path = mem::replace(&mut file_guard.path, PathBuf::new());
            drop(file_guard);
            shared_guard.file_ids_by_path.remove(&path);
            shared_guard.files.remove(&file_id);
        } else {
            // Otherwise, we can just drop the lock for the file.
            drop(file_guard);
        }
        
        // It's now safe to drop our remaining locks.
        drop(shared_guard);
        
        Ok(file_id)
    }
}

/// A trait for sending notifications over a connection.
pub trait NotificationSender: Send {
    /// This method is necessary to create clones of boxed trait objects.
    fn box_clone(&self) -> Box<dyn NotificationSender>;
    
    /// This method is called to send a notification over the corresponding connection.
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
    // The path to this file on the disk
    path: PathBuf,
    // The current revision of the file
    our_revision: u32,
    // The current contents of this file
    text: Text,
    // The list of deltas that has been seen by the server, but not yet by *every* client.
    outstanding_deltas: VecDeque<Delta>,
    // A map from connection ids to the participants for this file.
    participants_by_connection_id: HashMap<ConnectionId, Participant>,
}

impl File {
    // Sends the given `notification` except for the one with the given `connection_id`. This is
    // usually the participant that sent the request that caused this notification to happen in
    // the first place (so there's no need to notify it that something happened).
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

// Information about a participant
#[derive(Debug)]
struct Participant {
    // The last revision that has been seen by this participant.
    their_revision: u32,
    // Used to send notifications to (the connection of) this participant.
    notification_sender: Box<dyn NotificationSender>,
}
