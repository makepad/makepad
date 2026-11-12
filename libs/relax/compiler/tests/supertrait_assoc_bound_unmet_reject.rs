// An impl of a trait whose supertrait list bounds an associated type (`trait B: A<X: Copy>`)
// must meet that bound, directly and through a further supertrait (relax issue 10).
// rejects: E0277
trait A { type X; }
trait B: A<X: Copy> {}
trait C: B {}
struct S;
impl A for S { type X = String; }
impl B for S {}
impl C for S {}
