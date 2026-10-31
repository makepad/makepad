// rustc method/probe.rs: at each autoderef step, probe T, &T, then &mut T.
// Inherent methods precede trait methods only for the same adjusted receiver.

trait Shared {
    fn pick(&self) -> u32;
}

struct MutableInherent;
impl MutableInherent {
    fn pick(&mut self) -> u32 { 1 }
}
impl Shared for MutableInherent {
    fn pick(&self) -> u32 { 2 }
}

#[test]
fn shared_trait_precedes_mutable_inherent() {
    let mut value = MutableInherent;
    assert_eq!(value.pick(), 2);
    assert_eq!((&value).pick(), 2);
    // At &mut T itself, the inherent receiver matches by value.
    assert_eq!((&mut value).pick(), 1);
    assert_eq!(MutableInherent::pick(&mut value), 1);
}

trait ByValue {
    fn pick(self) -> u32;
}

struct SharedInherent;
impl SharedInherent {
    fn pick(&self) -> u32 { 3 }
}
impl ByValue for &SharedInherent {
    fn pick(self) -> u32 { 4 }
}

#[test]
fn inherent_and_trait_match_the_same_reference_receiver() {
    let value = SharedInherent;
    assert_eq!(value.pick(), 3);
    assert_eq!((&value).pick(), 3);
    assert_eq!(ByValue::pick(&value), 4);
}

struct ValueInherent;
impl ValueInherent {
    fn pick(self) -> u32 { 5 }
}
impl Shared for ValueInherent {
    fn pick(&self) -> u32 { 6 }
}

#[test]
fn inherent_by_value_precedes_shared_trait() {
    assert_eq!(ValueInherent.pick(), 5);
    // The &T step must not pick T's by-value inherent method.
    assert_eq!((&ValueInherent).pick(), 6);
}

trait Mutable {
    fn pick(&mut self) -> u32;
}

struct BothMutable;
impl BothMutable {
    fn pick(&mut self) -> u32 { 7 }
}
impl Mutable for BothMutable {
    fn pick(&mut self) -> u32 { 8 }
}

#[test]
fn inherent_precedes_trait_for_mutable_autoref() {
    let mut value = BothMutable;
    assert_eq!(value.pick(), 7);
    assert_eq!((&mut value).pick(), 7);
    assert_eq!(Mutable::pick(&mut value), 8);
}

struct View(u32);
impl View {
    fn child(&mut self, name: u32, style: u32) -> &Self {
        self.0 = name + style;
        self
    }
}
trait WidgetNode {
    fn child(&self, id: u32) -> &View;
}
impl WidgetNode for View {
    fn child(&self, id: u32) -> &View {
        assert_eq!(id, self.0);
        self
    }
}

#[test]
fn chained_child_uses_shared_trait_at_each_call() {
    let view = View(9);
    assert_eq!(view.child(9).child(9).0, 9);
    let mut other = View(0);
    assert_eq!(View::child(&mut other, 4, 5).child(9).0, 9);
}

struct BoxedInherent;
impl BoxedInherent {
    fn pick(self: Box<Self>) -> u32 { 10 }
}
impl Shared for BoxedInherent {
    fn pick(&self) -> u32 { 11 }
}

#[test]
fn typed_receiver_matches_only_its_own_step() {
    assert_eq!(Box::new(BoxedInherent).pick(), 10);
    // A Box<Self> receiver is not a by-value candidate at the Self step.
    assert_eq!(BoxedInherent.pick(), 11);
}

struct BorrowedBox;
impl BorrowedBox {
    fn pick(self: &Box<Self>) -> u32 { 15 }
}
impl ByValue for Box<BorrowedBox> {
    fn pick(self) -> u32 { 16 }
}

#[test]
fn typed_shared_receiver_waits_for_autoref() {
    assert_eq!(Box::new(BorrowedBox).pick(), 16);
    assert_eq!(BorrowedBox::pick(&Box::new(BorrowedBox)), 15);
}

struct ReferenceTrait;
impl ReferenceTrait {
    fn pick(&mut self) -> u32 { 13 }
}
impl ByValue for &ReferenceTrait {
    fn pick(self) -> u32 { 14 }
}

#[test]
fn by_value_trait_on_reference_precedes_mutable_autoref() {
    let mut value = ReferenceTrait;
    assert_eq!(value.pick(), 14);
    assert_eq!((&value).pick(), 14);
    assert_eq!(ReferenceTrait::pick(&mut value), 13);
}
