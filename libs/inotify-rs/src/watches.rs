use std::{
    cmp::Ordering,
    ffi::CString,
    hash::{
        Hash,
        Hasher,
    },
    io,
    os::raw::c_int,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{
        Arc,
        Weak,
    },
};

use crate::inotify_sys as ffi;

use crate::fd_guard::FdGuard;

bitflags! {
    /// Describes a file system watch
    ///
    /// Passed to [`Watches::add`], to describe what file system events
    /// to watch for, and how to do that.
    ///
    /// # Examples
    ///
    /// `WatchMask` constants can be passed to [`Watches::add`] as is. For
    /// example, here's how to create a watch that triggers an event when a file
    /// is accessed:
    ///
    /// ``` rust
    /// # use inotify::{
    /// #     Inotify,
    /// #     WatchMask,
    /// # };
    /// #
    /// # let mut inotify = Inotify::init().unwrap();
    /// #
    /// # // Create a temporary file, so `Watches::add` won't return an error.
    /// # use std::fs::File;
    /// # File::create("/tmp/inotify-rs-test-file")
    /// #     .expect("Failed to create test file");
    /// #
    /// inotify.watches().add("/tmp/inotify-rs-test-file", WatchMask::ACCESS)
    ///    .expect("Error adding watch");
    /// ```
    ///
    /// You can also combine multiple `WatchMask` constants. Here we add a watch
    /// this is triggered both when files are created or deleted in a directory:
    ///
    /// ``` rust
    /// # use inotify::{
    /// #     Inotify,
    /// #     WatchMask,
    /// # };
    /// #
    /// # let mut inotify = Inotify::init().unwrap();
    /// inotify.watches().add("/tmp/", WatchMask::CREATE | WatchMask::DELETE)
    ///    .expect("Error adding watch");
    /// ```
    ///
    /// [`Watches::add`]: struct.Watches.html#method.add
    pub struct WatchMask: u32 {
        /// File was accessed
        ///
        /// When watching a directory, this event is only triggered for objects
        /// inside the directory, not the directory itself.
        ///
        /// See [`inotify_sys::IN_ACCESS`].
        ///
        /// [`inotify_sys::IN_ACCESS`]: ../inotify_sys/constant.IN_ACCESS.html
        const ACCESS = ffi::IN_ACCESS;

        /// Metadata (permissions, timestamps, ...) changed
        ///
        /// When watching a directory, this event can be triggered for the
        /// directory itself, as well as objects inside the directory.
        ///
        /// See [`inotify_sys::IN_ATTRIB`].
        ///
        /// [`inotify_sys::IN_ATTRIB`]: ../inotify_sys/constant.IN_ATTRIB.html
        const ATTRIB = ffi::IN_ATTRIB;

        /// File opened for writing was closed
        ///
        /// When watching a directory, this event is only triggered for objects
        /// inside the directory, not the directory itself.
        ///
        /// See [`inotify_sys::IN_CLOSE_WRITE`].
        ///
        /// [`inotify_sys::IN_CLOSE_WRITE`]: ../inotify_sys/constant.IN_CLOSE_WRITE.html
        const CLOSE_WRITE = ffi::IN_CLOSE_WRITE;

        /// File or directory not opened for writing was closed
        ///
        /// When watching a directory, this event can be triggered for the
        /// directory itself, as well as objects inside the directory.
        ///
        /// See [`inotify_sys::IN_CLOSE_NOWRITE`].
        ///
        /// [`inotify_sys::IN_CLOSE_NOWRITE`]: ../inotify_sys/constant.IN_CLOSE_NOWRITE.html
        const CLOSE_NOWRITE = ffi::IN_CLOSE_NOWRITE;

        /// File/directory created in watched directory
        ///
        /// When watching a directory, this event is only triggered for objects
        /// inside the directory, not the directory itself.
        ///
        /// See [`inotify_sys::IN_CREATE`].
        ///
        /// [`inotify_sys::IN_CREATE`]: ../inotify_sys/constant.IN_CREATE.html
        const CREATE = ffi::IN_CREATE;

        /// File/directory deleted from watched directory
        ///
        /// When watching a directory, this event is only triggered for objects
        /// inside the directory, not the directory itself.
        ///
        /// See [`inotify_sys::IN_DELETE`].
        ///
        /// [`inotify_sys::IN_DELETE`]: ../inotify_sys/constant.IN_DELETE.html
        const DELETE = ffi::IN_DELETE;

        /// Watched file/directory was deleted
        ///
        /// See [`inotify_sys::IN_DELETE_SELF`].
        ///
        /// [`inotify_sys::IN_DELETE_SELF`]: ../inotify_sys/constant.IN_DELETE_SELF.html
        const DELETE_SELF = ffi::IN_DELETE_SELF;

        /// File was modified
        ///
        /// When watching a directory, this event is only triggered for objects
        /// inside the directory, not the directory itself.
        ///
        /// See [`inotify_sys::IN_MODIFY`].
        ///
        /// [`inotify_sys::IN_MODIFY`]: ../inotify_sys/constant.IN_MODIFY.html
        const MODIFY = ffi::IN_MODIFY;

        /// Watched file/directory was moved
        ///
        /// See [`inotify_sys::IN_MOVE_SELF`].
        ///
        /// [`inotify_sys::IN_MOVE_SELF`]: ../inotify_sys/constant.IN_MOVE_SELF.html
        const MOVE_SELF = ffi::IN_MOVE_SELF;

        /// File was renamed/moved; watched directory contained old name
        ///
        /// When watching a directory, this event is only triggered for objects
        /// inside the directory, not the directory itself.
        ///
        /// See [`inotify_sys::IN_MOVED_FROM`].
        ///
        /// [`inotify_sys::IN_MOVED_FROM`]: ../inotify_sys/constant.IN_MOVED_FROM.html
        const MOVED_FROM = ffi::IN_MOVED_FROM;

        /// File was renamed/moved; watched directory contains new name
        ///
        /// When watching a directory, this event is only triggered for objects
        /// inside the directory, not the directory itself.
        ///
        /// See [`inotify_sys::IN_MOVED_TO`].
        ///
        /// [`inotify_sys::IN_MOVED_TO`]: ../inotify_sys/constant.IN_MOVED_TO.html
        const MOVED_TO = ffi::IN_MOVED_TO;

        /// File or directory was opened
        ///
        /// When watching a directory, this event can be triggered for the
        /// directory itself, as well as objects inside the directory.
        ///
        /// See [`inotify_sys::IN_OPEN`].
        ///
        /// [`inotify_sys::IN_OPEN`]: ../inotify_sys/constant.IN_OPEN.html
        const OPEN = ffi::IN_OPEN;

        /// Watch for all events
        ///
        /// This constant is simply a convenient combination of the following
        /// other constants:
        ///
        /// - [`ACCESS`]
        /// - [`ATTRIB`]
        /// - [`CLOSE_WRITE`]
        /// - [`CLOSE_NOWRITE`]
        /// - [`CREATE`]
        /// - [`DELETE`]
        /// - [`DELETE_SELF`]
        /// - [`MODIFY`]
        /// - [`MOVE_SELF`]
        /// - [`MOVED_FROM`]
        /// - [`MOVED_TO`]
        /// - [`OPEN`]
        ///
        /// See [`inotify_sys::IN_ALL_EVENTS`].
        ///
        /// [`ACCESS`]: #associatedconstant.ACCESS
        /// [`ATTRIB`]: #associatedconstant.ATTRIB
        /// [`CLOSE_WRITE`]: #associatedconstant.CLOSE_WRITE
        /// [`CLOSE_NOWRITE`]: #associatedconstant.CLOSE_NOWRITE
        /// [`CREATE`]: #associatedconstant.CREATE
        /// [`DELETE`]: #associatedconstant.DELETE
        /// [`DELETE_SELF`]: #associatedconstant.DELETE_SELF
        /// [`MODIFY`]: #associatedconstant.MODIFY
        /// [`MOVE_SELF`]: #associatedconstant.MOVE_SELF
        /// [`MOVED_FROM`]: #associatedconstant.MOVED_FROM
        /// [`MOVED_TO`]: #associatedconstant.MOVED_TO
        /// [`OPEN`]: #associatedconstant.OPEN
        /// [`inotify_sys::IN_ALL_EVENTS`]: ../inotify_sys/constant.IN_ALL_EVENTS.html
        const ALL_EVENTS = ffi::IN_ALL_EVENTS;

        /// Watch for all move events
        ///
        /// This constant is simply a convenient combination of the following
        /// other constants:
        ///
        /// - [`MOVED_FROM`]
        /// - [`MOVED_TO`]
        ///
        /// See [`inotify_sys::IN_MOVE`].
        ///
        /// [`MOVED_FROM`]: #associatedconstant.MOVED_FROM
        /// [`MOVED_TO`]: #associatedconstant.MOVED_TO
        /// [`inotify_sys::IN_MOVE`]: ../inotify_sys/constant.IN_MOVE.html
        const MOVE = ffi::IN_MOVE;

        /// Watch for all close events
        ///
        /// This constant is simply a convenient combination of the following
        /// other constants:
        ///
        /// - [`CLOSE_WRITE`]
        /// - [`CLOSE_NOWRITE`]
        ///
        /// See [`inotify_sys::IN_CLOSE`].
        ///
        /// [`CLOSE_WRITE`]: #associatedconstant.CLOSE_WRITE
        /// [`CLOSE_NOWRITE`]: #associatedconstant.CLOSE_NOWRITE
        /// [`inotify_sys::IN_CLOSE`]: ../inotify_sys/constant.IN_CLOSE.html
        const CLOSE = ffi::IN_CLOSE;

        /// Don't dereference the path if it is a symbolic link
        ///
        /// See [`inotify_sys::IN_DONT_FOLLOW`].
        ///
        /// [`inotify_sys::IN_DONT_FOLLOW`]: ../inotify_sys/constant.IN_DONT_FOLLOW.html
        const DONT_FOLLOW = ffi::IN_DONT_FOLLOW;

        /// Filter events for directory entries that have been unlinked
        ///
        /// See [`inotify_sys::IN_EXCL_UNLINK`].
        ///
        /// [`inotify_sys::IN_EXCL_UNLINK`]: ../inotify_sys/constant.IN_EXCL_UNLINK.html
        const EXCL_UNLINK = ffi::IN_EXCL_UNLINK;

        /// If a watch for the inode exists, amend it instead of replacing it
        ///
        /// See [`inotify_sys::IN_MASK_ADD`].
        ///
        /// [`inotify_sys::IN_MASK_ADD`]: ../inotify_sys/constant.IN_MASK_ADD.html
        const MASK_ADD = ffi::IN_MASK_ADD;

        /// Only receive one event, then remove the watch
        ///
        /// See [`inotify_sys::IN_ONESHOT`].
        ///
        /// [`inotify_sys::IN_ONESHOT`]: ../inotify_sys/constant.IN_ONESHOT.html
        const ONESHOT = ffi::IN_ONESHOT;

        /// Only watch path, if it is a directory
        ///
        /// See [`inotify_sys::IN_ONLYDIR`].
        ///
        /// [`inotify_sys::IN_ONLYDIR`]: ../inotify_sys/constant.IN_ONLYDIR.html
        const ONLYDIR = ffi::IN_ONLYDIR;
    }
}

impl WatchDescriptor {
    /// Getter method for a watcher's id.
    ///
    /// Can be used to distinguish events for files with the same name.
    pub fn get_watch_descriptor_id(&self) -> c_int {
        self.id
    }
}

/// Interface for adding and removing watches
#[derive(Clone, Debug)]
pub struct Watches {
    pub(crate) fd: Arc<FdGuard>,
}

impl Watches {
    /// Init watches with an inotify file descriptor
    pub(crate) fn new(fd: Arc<FdGuard>) -> Self {
        Watches {
            fd,
        }
    }

    /// Adds or updates a watch for the given path
    ///
    /// Adds a new watch or updates an existing one for the file referred to by
    /// `path`. Returns a watch descriptor that can be used to refer to this
    /// watch later.
    ///
    /// The `mask` argument defines what kind of changes the file should be
    /// watched for, and how to do that. See the documentation of [`WatchMask`]
    /// for details.
    ///
    /// If this method is used to add a new watch, a new [`WatchDescriptor`] is
    /// returned. If it is used to update an existing watch, a
    /// [`WatchDescriptor`] that equals the previously returned
    /// [`WatchDescriptor`] for that watch is returned instead.
    ///
    /// Under the hood, this method just calls [`inotify_add_watch`] and does
    /// some trivial translation between the types on the Rust side and the C
    /// side.
    ///
    /// # Attention: Updating watches and hardlinks
    ///
    /// As mentioned above, this method can be used to update an existing watch.
    /// This is usually done by calling this method with the same `path`
    /// argument that it has been called with before. But less obviously, it can
    /// also happen if the method is called with a different path that happens
    /// to link to the same inode.
    ///
    /// You can detect this by keeping track of [`WatchDescriptor`]s and the
    /// paths they have been returned for. If the same [`WatchDescriptor`] is
    /// returned for a different path (and you haven't freed the
    /// [`WatchDescriptor`] by removing the watch), you know you have two paths
    /// pointing to the same inode, being watched by the same watch.
    ///
    /// # Errors
    ///
    /// Directly returns the error from the call to
    /// [`inotify_add_watch`][`inotify_add_watch`] (translated into an
    /// `io::Error`), without adding any error conditions of
    /// its own.
    ///
    /// # Examples
    ///
    /// ```
    /// use inotify::{
    ///     Inotify,
    ///     WatchMask,
    /// };
    ///
    /// let mut inotify = Inotify::init()
    ///     .expect("Failed to initialize an inotify instance");
    ///
    /// # // Create a temporary file, so `Watches::add` won't return an error.
    /// # use std::fs::File;
    /// # File::create("/tmp/inotify-rs-test-file")
    /// #     .expect("Failed to create test file");
    /// #
    /// inotify.watches().add("/tmp/inotify-rs-test-file", WatchMask::MODIFY)
    ///     .expect("Failed to add file watch");
    ///
    /// // Handle events for the file here
    /// ```
    ///
    /// [`inotify_add_watch`]: ../inotify_sys/fn.inotify_add_watch.html
    /// [`WatchMask`]: struct.WatchMask.html
    /// [`WatchDescriptor`]: struct.WatchDescriptor.html
    pub fn add<P>(&mut self, path: P, mask: WatchMask)
                        -> io::Result<WatchDescriptor>
        where P: AsRef<Path>
    {
        let path = CString::new(path.as_ref().as_os_str().as_bytes())?;

        let wd = unsafe {
            ffi::inotify_add_watch(
                **self.fd,
                path.as_ptr() as *const _,
                mask.bits(),
            )
        };

        match wd {
            -1 => Err(io::Error::last_os_error()),
            _  => Ok(WatchDescriptor{ id: wd, fd: Arc::downgrade(&self.fd) }),
        }
    }

    /// Stops watching a file
    ///
    /// Removes the watch represented by the provided [`WatchDescriptor`] by
    /// calling [`inotify_rm_watch`]. [`WatchDescriptor`]s can be obtained via
    /// [`Watches::add`], or from the `wd` field of [`Event`].
    ///
    /// # Errors
    ///
    /// Directly returns the error from the call to [`inotify_rm_watch`].
    /// Returns an [`io::Error`] with [`ErrorKind`]`::InvalidInput`, if the given
    /// [`WatchDescriptor`] did not originate from this [`Inotify`] instance.
    ///
    /// # Examples
    ///
    /// ```
    /// use inotify::Inotify;
    ///
    /// let mut inotify = Inotify::init()
    ///     .expect("Failed to initialize an inotify instance");
    ///
    /// # // Create a temporary file, so `Watches::add` won't return an error.
    /// # use std::fs::File;
    /// # let mut test_file = File::create("/tmp/inotify-rs-test-file")
    /// #     .expect("Failed to create test file");
    /// #
    /// # // Add a watch and modify the file, so the code below doesn't block
    /// # // forever.
    /// # use inotify::WatchMask;
    /// # inotify.watches().add("/tmp/inotify-rs-test-file", WatchMask::MODIFY)
    /// #     .expect("Failed to add file watch");
    /// # use std::io::Write;
    /// # write!(&mut test_file, "something\n")
    /// #     .expect("Failed to write something to test file");
    /// #
    /// let mut buffer = [0; 1024];
    /// let events = inotify
    ///     .read_events_blocking(&mut buffer)
    ///     .expect("Error while waiting for events");
    /// let mut watches = inotify.watches();
    ///
    /// for event in events {
    ///     watches.remove(event.wd);
    /// }
    /// ```
    ///
    /// [`WatchDescriptor`]: struct.WatchDescriptor.html
    /// [`inotify_rm_watch`]: ../inotify_sys/fn.inotify_rm_watch.html
    /// [`Watches::add`]: struct.Watches.html#method.add
    /// [`Event`]: struct.Event.html
    /// [`Inotify`]: struct.Inotify.html
    /// [`io::Error`]: https://doc.rust-lang.org/std/io/struct.Error.html
    /// [`ErrorKind`]: https://doc.rust-lang.org/std/io/enum.ErrorKind.html
    pub fn remove(&mut self, wd: WatchDescriptor) -> io::Result<()> {
        if wd.fd.upgrade().as_ref() != Some(&self.fd) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid WatchDescriptor",
            ));
        }

        let result = unsafe { ffi::inotify_rm_watch(**self.fd, wd.id) };
        match result {
            0  => Ok(()),
            -1 => Err(io::Error::last_os_error()),
            _  => panic!(
                "unexpected return code from inotify_rm_watch ({})", result)
        }
    }
}


/// Represents a watch on an inode
///
/// Can be obtained from [`Watches::add`] or from an [`Event`]. A watch
/// descriptor can be used to get inotify to stop watching an inode by passing
/// it to [`Watches::remove`].
///
/// [`Watches::add`]: struct.Watches.html#method.add
/// [`Watches::remove`]: struct.Watches.html#method.remove
/// [`Event`]: struct.Event.html
#[derive(Clone, Debug)]
pub struct WatchDescriptor{
    pub(crate) id: c_int,
    pub(crate) fd: Weak<FdGuard>,
}

impl Eq for WatchDescriptor {}

impl PartialEq for WatchDescriptor {
    fn eq(&self, other: &Self) -> bool {
        let self_fd  = self.fd.upgrade();
        let other_fd = other.fd.upgrade();

        self.id == other.id && self_fd.is_some() && self_fd == other_fd
    }
}

impl Ord for WatchDescriptor {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for WatchDescriptor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for WatchDescriptor {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // This function only takes `self.id` into account, as `self.fd` is a
        // weak pointer that might no longer be available. Since neither
        // panicking nor changing the hash depending on whether it's available
        // is acceptable, we just don't look at it at all.
        // I don't think that this influences storage in a `HashMap` or
        // `HashSet` negatively, as storing `WatchDescriptor`s from different
        // `Inotify` instances seems like something of an anti-pattern anyway.
        self.id.hash(state);
    }
}
