use array_vec::{ArrayVec, CapacityError};

#[test]
fn test_push() {
    let mut array_vec = ArrayVec::<_, 4>::new();
    array_vec.push(0);
    assert_eq!(&array_vec[..], &[0]);
    array_vec.push(1);
    assert_eq!(&array_vec[..], &[0, 1]);
    array_vec.push(2);
    assert_eq!(&array_vec[..], &[0, 1, 2]);
    array_vec.push(3);
    assert_eq!(&array_vec[..], &[0, 1, 2, 3]);
    assert_eq!(array_vec.try_push(4), Err(CapacityError));
}

#[test]
fn test_insert() {
    let mut array_vec = ArrayVec::<_, 4>::new();
    array_vec.insert(0, 2);
    assert_eq!(&array_vec[..], &[2]);
    array_vec.insert(0, 0);
    assert_eq!(&array_vec[..], &[0, 2]);
    array_vec.insert(1, 1);
    assert_eq!(&array_vec[..], &[0, 1, 2]);
    array_vec.insert(3, 3);
    assert_eq!(&array_vec[..], &[0, 1, 2, 3]);
    assert_eq!(array_vec.try_insert(4, 4), Err(CapacityError));
}

#[test]
fn test_extend_from_slice() {
    let mut array_vec = ArrayVec::<_, 8>::new();
    array_vec.extend_from_slice(&[0, 1, 2, 3]);
    array_vec.extend_from_slice(&[4, 5, 6, 7]);
    assert_eq!(&array_vec[..], &[0, 1, 2, 3, 4, 5, 6, 7]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    assert_eq!(
        array_vec.try_extend_from_slice(&[4, 5, 6, 7]),
        Err(CapacityError)
    );
}

#[test]
fn test_pop() {
    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    assert_eq!(array_vec.pop(), Some(3));
    assert_eq!(&array_vec[..], &[0, 1, 2]);
    assert_eq!(array_vec.pop(), Some(2));
    assert_eq!(&array_vec[..], &[0, 1]);
    assert_eq!(array_vec.pop(), Some(1));
    assert_eq!(&array_vec[..], &[0]);
    assert_eq!(array_vec.pop(), Some(0));
    assert_eq!(&array_vec[..], &[]);
    assert_eq!(array_vec.pop(), None);
}

#[test]
fn test_remove() {
    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    assert_eq!(array_vec.remove(3), 3);
    assert_eq!(&array_vec[..], &[0, 1, 2]);
    assert_eq!(array_vec.remove(1), 1);
    assert_eq!(&array_vec[..], &[0, 2]);
    assert_eq!(array_vec.remove(0), 0);
    assert_eq!(&array_vec[..], &[2]);
    assert_eq!(array_vec.remove(0), 2);
}

#[test]
fn test_splice() {
    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(0..0, []);
    assert_eq!(&array_vec[..], &[0, 1, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(0..1, [4]);
    assert_eq!(&array_vec[..], &[4, 1, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(1..2, [4]);
    assert_eq!(&array_vec[..], &[0, 4, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(2..3, [4]);
    assert_eq!(&array_vec[..], &[0, 1, 4, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(3..4, [4]);
    assert_eq!(&array_vec[..], &[0, 1, 2, 4]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(0..2, [4, 5]);
    assert_eq!(&array_vec[..], &[4, 5, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(1..3, [4, 5]);
    assert_eq!(&array_vec[..], &[0, 4, 5, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(2..4, [4, 5]);
    assert_eq!(&array_vec[..], &[0, 1, 4, 5]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.splice(0..4, [4, 5, 6, 7]);
    assert_eq!(&array_vec[..], &[4, 5, 6, 7]);

    let mut array_vec = ArrayVec::<_, 6>::new();
    array_vec.extend_from_slice(&[0, 1, 2, 3]);
    array_vec.splice(1..3, [4, 5, 6, 7]);
    assert_eq!(&array_vec[..], &[0, 4, 5, 6, 7, 3]);

    let mut array_vec = ArrayVec::<_, 6>::new();
    array_vec.extend_from_slice(&[0, 1, 2, 3]);
    array_vec.splice(1..3, [4]);
    assert_eq!(&array_vec[..], &[0, 4, 3]);
}

#[test]
fn test_drain() {
    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(0..0);
    assert_eq!(&array_vec[..], &[0, 1, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(0..1);
    assert_eq!(&array_vec[..], &[1, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(1..2);
    assert_eq!(&array_vec[..], &[0, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(2..3);
    assert_eq!(&array_vec[..], &[0, 1, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(3..4);
    assert_eq!(&array_vec[..], &[0, 1, 2]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(0..2);
    assert_eq!(&array_vec[..], &[2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(1..3);
    assert_eq!(&array_vec[..], &[0, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(2..4);
    assert_eq!(&array_vec[..], &[0, 1]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.drain(0..4);
    assert_eq!(&array_vec[..], &[]);
}

#[test]
fn test_truncate() {
    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.truncate(4);
    assert_eq!(&array_vec[..], &[0, 1, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.truncate(3);
    assert_eq!(&array_vec[..], &[0, 1, 2]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.truncate(2);
    assert_eq!(&array_vec[..], &[0, 1]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.truncate(1);
    assert_eq!(&array_vec[..], &[0]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.truncate(0);
    assert_eq!(&array_vec[..], &[]);
}

#[test]
fn test_clear() {
    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    array_vec.clear();
    assert_eq!(&array_vec[..], &[]);
}

#[test]
fn test_split_off() {
    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    let other_array_vec = array_vec.split_off(0);
    assert_eq!(&array_vec[..], &[]);
    assert_eq!(&other_array_vec[..], &[0, 1, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    let other_array_vec = array_vec.split_off(1);
    assert_eq!(&array_vec[..], &[0]);
    assert_eq!(&other_array_vec[..], &[1, 2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    let other_array_vec = array_vec.split_off(2);
    assert_eq!(&array_vec[..], &[0, 1]);
    assert_eq!(&other_array_vec[..], &[2, 3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    let other_array_vec = array_vec.split_off(3);
    assert_eq!(&array_vec[..], &[0, 1, 2]);
    assert_eq!(&other_array_vec[..], &[3]);

    let mut array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    let other_array_vec = array_vec.split_off(4);
    assert_eq!(&array_vec[..], &[0, 1, 2, 3]);
    assert_eq!(&other_array_vec[..], &[]);
}

#[test]
fn test_clone() {
    let array_vec: ArrayVec<_, 4> = [0, 1, 2, 3].into();
    let cloned_array_vec = array_vec.clone();
    assert_eq!(&cloned_array_vec[..], &[0, 1, 2, 3]);
}
