// A method call on a type-parameter receiver binds to the trait item from the bounds
// (param env), never to an inherent method of the concrete type the generic is
// instantiated with; a fully qualified `Trait::method` / `<W as Trait>::method` always
// names the trait item. Here `Belt` has an inherent `apply` with a different signature
// next to its `Styled::apply` impl (the Styled::apply / glue.rs new_styled shape).

#[derive(Clone, Copy, PartialEq, Debug)]
enum ApplyKind { Full, Partial }

#[derive(Clone, Copy, PartialEq, Debug)]
enum Command { Left, Right }

trait Styled: Sized + 'static {
    fn apply(&mut self, cx: &mut u32, kind: ApplyKind);
}

trait StyleWidget: Styled {
    fn defaults() -> Self;
}

struct Belt { styled: Vec<ApplyKind>, commands: Vec<Command> }

impl Belt {
    fn apply(&mut self, cx: &mut u32, command: Command) {
        *cx += 100;
        self.commands.push(command);
    }
}

impl Styled for Belt {
    fn apply(&mut self, cx: &mut u32, kind: ApplyKind) {
        *cx += 1;
        self.styled.push(kind);
    }
}

impl StyleWidget for Belt {
    fn defaults() -> Self { Belt { styled: Vec::new(), commands: Vec::new() } }
}

fn new_styled<W: StyleWidget>(cx: &mut u32) -> W {
    let mut value = W::defaults();
    value.apply(cx, ApplyKind::Full);
    value
}

fn qualified<W: StyleWidget>(cx: &mut u32, v: &mut W) {
    Styled::apply(v, cx, ApplyKind::Partial);
    <W as Styled>::apply(v, cx, ApplyKind::Full);
}

#[test]
fn type_param_receiver_binds_trait_method() {
    let mut cx = 0u32;
    let belt: Belt = new_styled(&mut cx);
    assert_eq!(cx, 1);
    assert_eq!(belt.styled, vec![ApplyKind::Full]);
    assert!(belt.commands.is_empty());
}

#[test]
fn qualified_trait_paths_bind_trait_method() {
    let mut cx = 0u32;
    let mut belt = Belt::defaults();
    qualified(&mut cx, &mut belt);
    // Concrete-type qualified paths too.
    Styled::apply(&mut belt, &mut cx, ApplyKind::Full);
    <Belt as Styled>::apply(&mut belt, &mut cx, ApplyKind::Partial);
    assert_eq!(cx, 4);
    assert_eq!(belt.styled, vec![ApplyKind::Partial, ApplyKind::Full, ApplyKind::Full, ApplyKind::Partial]);
    // The inherent method still wins for a plain call on the concrete type.
    belt.apply(&mut cx, Command::Left);
    Belt::apply(&mut belt, &mut cx, Command::Right);
    assert_eq!(cx, 204);
    assert_eq!(belt.commands, vec![Command::Left, Command::Right]);
}
