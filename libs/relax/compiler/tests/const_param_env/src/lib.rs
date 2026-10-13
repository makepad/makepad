// Reduced from code_graph::prepared_graph::builder: retain the nested
// closures, reference-taking calls, array copies, and scoped channel handoff.
pub struct ExternalSort<const N: usize>(pub Vec<[u8; N]>);

impl<const N: usize> ExternalSort<N> {
    pub fn finish(self, mut emit: impl FnMut(&[u8; N]) -> Result<(), &'static str>) -> Result<usize, &'static str> {
        for row in &self.0 {
            emit(row)?;
        }
        Ok(self.0.len())
    }
}

struct TableWriter(Vec<Vec<u8>>);

impl TableWriter {
    fn push(&mut self, row: &[u8]) {
        self.0.push(row.to_vec());
    }
}

pub fn indexed<const N: usize>(sort: ExternalSort<N>, dedupe: usize) -> Result<Vec<Vec<u8>>, &'static str> {
    const BATCH: usize = 2;
    let mut table = TableWriter(Vec::new());
    std::thread::scope(|scope| -> Result<(), &'static str> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<[u8; N]>>(4);
        let merger = scope.spawn(move || -> Result<usize, &'static str> {
            let mut previous: Option<[u8; N]> = None;
            let mut batch = Vec::with_capacity(BATCH);
            let stats = sort.finish(|row| {
                if previous.as_ref().is_some_and(|p| p[..dedupe] == row[..dedupe]) {
                    return Ok(());
                }
                previous = Some(*row);
                batch.push(*row);
                if batch.len() == BATCH {
                    tx.send(std::mem::replace(&mut batch, Vec::with_capacity(BATCH)))
                        .map_err(|_| "writer stopped")?;
                }
                Ok(())
            })?;
            if !batch.is_empty() {
                tx.send(batch).map_err(|_| "writer stopped")?;
            }
            Ok(stats)
        });
        for rows in rx {
            for row in &rows {
                table.push(row);
            }
        }
        merger.join().map_err(|_| "merge panicked")??;
        Ok(())
    })?;
    Ok(table.0)
}

pub struct Rows<const N: usize>(pub Vec<[u8; N]>);

impl<const N: usize> Rows<N> {
    pub fn bytes(&self) -> usize {
        self.0.capacity() * N
    }
}

pub struct Feed<'s, const N: usize> {
    tx: std::sync::mpsc::SyncSender<Vec<[u8; N]>>,
    handle: std::thread::ScopedJoinHandle<'s, ()>,
}

impl<'s, const N: usize> Feed<'s, N> {
    pub fn start<'e>(scope: &'s std::thread::Scope<'s, 'e>, sort: &'s mut ExternalSort<N>) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<[u8; N]>>(4);
        let handle = scope.spawn(move || {
            for rows in rx {
                for row in rows {
                    sort.0.push(row);
                }
            }
        });
        Feed { tx, handle }
    }

    pub fn send(&self, rows: Vec<[u8; N]>) {
        self.tx.send(rows).unwrap();
    }

    pub fn join(self) {
        drop(self.tx);
        self.handle.join().unwrap();
    }
}

fn copy_row<const M: usize>(row: &[u8; M]) -> [u8; M] {
    // Field-view analysis instantiates this body in the caller's parameter
    // space: M becomes N, whose index is 1 (index 0 is a bool).
    Some(*row).unwrap()
}

fn copy_value<T: Copy>(value: &T) -> T {
    Some(*value).unwrap()
}

pub fn forwarded<const TAG: bool, const N: usize>(row: [u8; N]) -> ([u8; N], [u8; N]) {
    let copy = || (copy_row(&row), copy_value(&row));
    let _ = TAG;
    copy()
}

fn compare_value<T: PartialOrd>(a: &T, b: &T) -> bool {
    a == b || a < b
}

pub fn ordered<const N: usize>(a: [u8; N], b: [u8; N]) -> bool {
    let compare = || compare_value(&a, &b);
    compare()
}
