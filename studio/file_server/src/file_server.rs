use {
    crate::{
        makepad_file_protocol::{
            DirectoryEntry,
            FileNodeData,
            FileTreeData,
            FileError,
            FileNotification,
            FileRequest,
            FileResponse,
            SaveKind,
            SaveFileResponse,
            OpenFileResponse
        },
    },
    std::{
        thread,
        cmp::Ordering,
        fmt,
        fs,
        time::Duration,
        path::{Path, PathBuf},
        sync::{Arc, RwLock, Mutex},
    },
};

pub struct FileServer {
    // The id for the next connection
    next_connection_id: usize,
    // State that is shared between every connection
    shared: Arc<RwLock<Shared >>,
}

impl FileServer {
    /// Creates a new collab server rooted at the given path.
    pub fn new<P: Into<PathBuf >> (root_path: P) -> FileServer {
        FileServer {
            next_connection_id: 0,
            shared: Arc::new(RwLock::new(Shared {
                root_path: root_path.into(),
            })),
        }
    }
    
    /// Creates a new connection to this collab server, and returns a handle for the connection.
    ///
    /// The given `notification_sender` is called whenever the server wants to send a notification
    /// for this connection. The embedder is responsible for sending the notification.
    pub fn connect(&mut self, notification_sender: Box<dyn NotificationSender>) -> FileServerConnection {
        let connection_id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        FileServerConnection {
            _connection_id:connection_id,
            shared: self.shared.clone(),
            _notification_sender: notification_sender,
            open_files: Default::default(),
            stop_observation: Default::default()
        }
    }
}

/// A connection to a collab server.
pub struct FileServerConnection {
    // The id for this connection.
    _connection_id: ConnectionId,
    // State is shared between every connection.
    shared: Arc<RwLock<Shared >>,
    // Used to send notifications for this connection.
    _notification_sender: Box<dyn NotificationSender>,
    open_files: Arc<Mutex<Vec<(String, u64, Vec<u8>)>>>,
    stop_observation: Arc<Mutex<bool>>,
}

impl FileServerConnection {
    /// Handles the given `request` for this connection, and returns the corresponding response.
    ///
    /// The embedder is responsible for receiving requests, calling this method to handle them, and
    /// sending back the response.
    pub fn handle_request(&self, request: FileRequest) -> FileResponse {
        
        
        match request {
            FileRequest::LoadFileTree {with_data} => FileResponse::LoadFileTree(self.load_file_tree(with_data)),
            FileRequest::OpenFile{path,id} => FileResponse::OpenFile(self.open_file(path, id)),
            FileRequest::SaveFile{path, data, id, patch} => FileResponse::SaveFile(self.save_file(path, data, id, patch)),
        }
    }
    
    // Handles a `LoadFileTree` request.
    fn load_file_tree(&self, with_data: bool) -> Result<FileTreeData, FileError> {
        // A recursive helper function for traversing the entries of a directory and creating the
        // data structures that describe them.
        fn get_directory_entries(path: &Path, with_data: bool) -> Result<Vec<DirectoryEntry>, FileError> {
            let mut entries = Vec::new();
            for entry in fs::read_dir(path).map_err( | error | FileError::Unknown(error.to_string())) ? {
                // We can't get the entry for some unknown reason. Raise an error.
                let entry = entry.map_err( | error | FileError::Unknown(error.to_string())) ?;
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
                    name: entry.file_name().to_string_lossy().to_string(),
                    node: if entry_path.is_dir() {
                        // If this entry is a subdirectory, recursively create `DirectoryEntry`'s
                        // for its entries as well.
                        FileNodeData::Directory {
                            entries: get_directory_entries(&entry_path, with_data) ?,
                        }
                    } else if entry_path.is_file() {
                        if with_data {
                            let bytes: Vec<u8> = fs::read(&entry_path).map_err(
                                | error | FileError::Unknown(error.to_string())
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
                match &entry_0.node {
                    FileNodeData::Directory {..} => match &entry_1.node {
                        FileNodeData::Directory {..} => entry_0.name.cmp(&entry_1.name),
                        FileNodeData::File {..} => Ordering::Less
                    }
                    FileNodeData::File {..} => match &entry_1.node {
                        FileNodeData::Directory {..} => Ordering::Greater,
                        FileNodeData::File {..} => entry_0.name.cmp(&entry_1.name)
                    }
                }
            });
            Ok(entries)
        }
        
        let root_path = self.shared.read().unwrap().root_path.clone();
        
        let root = FileNodeData::Directory {
            entries: get_directory_entries(&root_path, with_data) ?,
        };
        Ok(FileTreeData {root_path: "".into(), root})
    }
    
    fn make_full_path(&self, child_path:&String)->PathBuf{
        let mut path = self.shared.read().unwrap().root_path.clone();
        path.push(child_path);
        path
    }
    
    fn start_observation(&self) {
        let open_files = self.open_files.clone();
        let shared = self.shared.clone();
        let notification_sender = self._notification_sender.clone();
        let stop_observation = self.stop_observation.clone();
        thread::spawn(move || {
            let stop = *stop_observation.lock().unwrap();
            while !stop{
                if let Ok(mut files) = open_files.lock(){
                    for (path, file_id, last_content) in files.iter_mut() {
                        let full_path = {
                            let shared = shared.read().unwrap();
                            shared.root_path.join(&path)
                        };
                        if let Ok(bytes) = fs::read(&full_path) {
                            if bytes.len() > 0 && bytes != *last_content {
                                let new_data = String::from_utf8_lossy(&bytes);
                                let old_data = String::from_utf8_lossy(&last_content);
                                // Send notification of external file change.
                                notification_sender
                                .send_notification(FileNotification::FileChangedOnDisk(
                                    SaveFileResponse{
                                        path: path.to_string(),
                                        new_data: new_data.to_string(),
                                        old_data: old_data.to_string(),
                                        kind: SaveKind::Observation,
                                        id: *file_id
                                    }
                                ));
                                *last_content = bytes;
                            }
                        }
                    }
                }
                // Sleep for 500ms.
                thread::sleep(Duration::from_millis(100));
            }
        });
    }
    
    // Handles an `OpenFile` request.
    fn open_file(&self, child_path: String, id:u64) -> Result<OpenFileResponse, FileError> {
        let path = self.make_full_path(&child_path);
        
        let bytes = fs::read(&path).map_err(
            | error | FileError::Unknown(error.to_string())
        ) ?;
        
        let mut open_files = self.open_files.lock().unwrap();
        
        if open_files.iter().find(|(cp,_,_)| *cp == child_path).is_none(){
            open_files.push((child_path.clone(), id, bytes.clone()));
        }
        
        if open_files.len() == 1 {
            self.start_observation();
        }
        // Converts the file contents to a `Text`. This is necessarily a lossy conversion
        // because `Text` assumes everything is UTF-8 encoded, and this isn't always the
        // case for files on disk (is this a problem?)
        /*let text: Text = Text::from_lines(String::from_utf8_lossy(&bytes)
            .lines()
            .map( | line | line.chars().collect::<Vec<_ >> ())
            .collect::<Vec<_ >>());*/
        
        let text = String::from_utf8_lossy(&bytes);
        Ok(OpenFileResponse{
            path: child_path,
            data: text.to_string(),
            id
        })
    }
    
    // Handles an `ApplyDelta` request.
    fn save_file(
        &self,
        child_path: String,
        new_data: String,
        id: u64,
        patch: bool
    ) -> Result<SaveFileResponse, FileError> {
        let mut open_files = self.open_files.lock().unwrap();
                
        if let Some(of) = open_files.iter_mut().find(|(cp,_,_)| *cp == child_path){
            of.2 =  new_data.as_bytes().to_vec();
        }
        else{
            open_files.push((child_path.clone(), id, new_data.as_bytes().to_vec()));
        }
        
        let path = self.make_full_path(&child_path);
        
        let old_data = String::from_utf8_lossy(&fs::read(&path).map_err(
            | error | FileError::Unknown(error.to_string())
        ) ?).to_string();

        fs::write(&path, &new_data).map_err(
            | error | FileError::Unknown(error.to_string())
        ) ?;
        
        Ok(SaveFileResponse{
            path: child_path, 
            old_data,
            new_data,
            id,
            kind: if patch{SaveKind::Patch}else{SaveKind::Save}
        })
    }
}

/// A trait for sending notifications over a connection.
pub trait NotificationSender: Send {
    /// This method is necessary to create clones of boxed trait objects.
    fn box_clone(&self) -> Box<dyn NotificationSender>;
    
    /// This method is called to send a notification over the corresponding connection.
    fn send_notification(&self, notification: FileNotification);
}

impl<F: Clone + Fn(FileNotification) + Send + 'static> NotificationSender for F {
    fn box_clone(&self) -> Box<dyn NotificationSender> {
        Box::new(self.clone())
    }
    
    fn send_notification(&self, notification: FileNotification) {
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
    root_path: PathBuf,
}

/// An identifier for a connection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionId(usize);

