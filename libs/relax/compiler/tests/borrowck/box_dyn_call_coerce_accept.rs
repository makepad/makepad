// egui containers/combo_box.rs: `icon(ui, rect, visuals, open)` where `icon: Box<dyn FnOnce(&Ui, ..)>`
// and `ui: &mut Ui` while `visuals` still borrows `ui`. The argument coerces `&mut Ui` to the
// `&Ui` input of the Fn* obligation (Box<F>: Fn<A>), a shared reborrow, not a mutable one.
pub struct Style { pub open: u32 }
pub struct Ui { style: Style, n: u32 }
impl Ui {
    fn visuals(&self) -> &Style { &self.style }
    fn style(&self) -> &Style { &self.style }
}
fn frame(ui: &mut Ui, f: impl FnOnce(&mut Ui)) { f(ui) }
fn once(ui: &mut Ui, icon: Option<Box<dyn FnOnce(&Ui, &Style)>>) {
    frame(ui, |ui| {
        let open = ui.n > 3;
        let visuals = if open { &ui.visuals() } else { ui.style() };
        if let Some(icon) = icon {
            icon(ui, visuals);
        }
        let _ = visuals.open;
    });
}
fn shared(ui: &mut Ui, icon: Box<dyn Fn(&Ui, &Style)>) {
    let visuals = ui.visuals();
    icon(ui, visuals);
    let _ = visuals.open;
}
fn main() {
    let mut ui = Ui { style: Style { open: 1 }, n: 4 };
    once(&mut ui, Some(Box::new(|u, s| println!("{} {}", u.n, s.open))));
    shared(&mut ui, Box::new(|u, s| println!("{} {}", u.n, s.open)));
}
