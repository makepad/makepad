
use {
    crate::{
        app::{AppData},
        file_system::file_system::SnapshotImageData,
        makepad_widgets::*,
    },
    std::{
        env,
    },
};

live_design!{
    use link::shaders::*;
    use link::widgets::*;
    use link::theme::*;
    use makepad_widgets::designer_theme::*;
    
    SnapshotItem = <RoundedView> {
        height: Fit, width: Fill
        draw_bg:{color:#5}
        flow:Down
        message = <Label>{text:"test"}
        image = <Image> {
            width: Fill,
            height: 300
            min_width: 1920,
            min_height: 1080,
            fit: Horizontal,
            draw_bg: {
                instance hover: 0.0
                instance down: 0.0
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(1, 1, self.rect_size.x - 2, self.rect_size.y - 2, 4.0)
                    let max_scale = vec2(0.92);
                    let scale = mix(vec2(1.0), max_scale, self.hover);
                    let pan = mix(vec2(0.0), (vec2(1.0) - max_scale) * 0.5, self.hover)* self.image_scale;
                    let color = self.get_color_scale_pan(scale * self.image_scale, pan + self.image_pan) + mix(vec4(0.0), vec4(0.1), self.down);
                    if color.a<0.0001{
                        color = #3
                    }
                    sdf.fill_keep(color);
                    sdf.stroke(
                        mix(mix(#x0000, #x0006, self.hover), #xfff2, self.down),
                        1.0
                    )
                                        
                    return sdf.result
                }
            }
        }
    }
    
    pub Snapshot = {{Snapshot}} <RectView> {
        height: Fill, width: Fill,
        //draw_bg: {color: (THEME_COLOR_BG_CONTAINER)}
        flow: Down,
        <DockToolbar> {
            height: 72
            content = {
                padding:{top:1}
                spacing: (THEME_SPACE_2)
                flow: Down
                <View>{
                    spacing: 5
                    roots_dropdown = <DropDownFlat>{ width: Fit, popup_menu_position: BelowInput }
                    <Button>{text:"Snapshot"}
                    <CheckBox>{text:"Auto"}
                }
                <TextInput>{empty_text:"Description"}
            }
        }
        list = <PortalList> {
            capture_overload: false,
            grab_key_focus: false
            auto_tail: true
            drag_scrolling: false
            max_pull_down: 0,
            height: Fill, width: Fill,
            flow: Down
            SnapshotItem = <SnapshotItem> {}
            Empty = <SolidView> {
                cursor: Default
                draw_bg:{color:#44}
                width: Fill
                height: 80,
            }
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SnapshotAction {
    Load(String),
    None
}

#[derive(Live, LiveHook, Widget)]
pub struct Snapshot{
    #[deref] view:View
}

impl Snapshot{
    fn draw_snapshots(&mut self, cx: &mut Cx2d, list:&mut PortalList, scope:&mut Scope, root_id:usize){
        let data = scope.data.get_mut::<AppData>().unwrap();
        let file_system = &mut data.file_system;
        let git_log = file_system.git_logs.get(root_id as usize).unwrap();
        list.set_item_range(cx, 0, git_log.commits.len());
        while let Some(item_id) = list.next_visible_item(cx) {
            let item = if let Some(commit) = git_log.commits.get(item_id){
                let item = list.item(cx, item_id, live_id!(SnapshotItem)).as_view();
                item.label(id!(message)).set_text(cx, &commit.message);
                // lets construct a snapshot image filepath from the commit message
                // check if we have a image path or not
                let image = item.image(id!(image));
                
                let load = match file_system.snapshot_image_data.borrow().get(&commit.hash){
                    Some(SnapshotImageData::Loading)=>{
                        image.set_visible(cx, true);
                        false
                    }
                    Some(SnapshotImageData::Error)=>{
                        image.set_visible(cx, false);
                        false
                    }
                    Some(SnapshotImageData::Loaded{data, path})=>{
                        image.set_visible(cx, true);
                        image.load_image_from_data_async(cx, &path, data.clone()).ok();
                        false
                    }
                    None=>true
                };
                if load{ 
                    file_system.file_client.load_snapshot_image(&git_log.root, &commit.hash);
                }
                item
            }
            else{
                list.item(cx, item_id, live_id!(Empty)).as_view()
            };
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for Snapshot {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        let data = scope.data.get_mut::<AppData>().unwrap();
        
        let dd = self.drop_down(id!(roots_dropdown));
        let mut i = data.file_system.git_logs.iter();
        dd.set_labels_with(cx, |label|{
            i.next().map(|m| label.push_str(&m.root));
        });
        let root_id = dd.selected_item();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step(){
            if let Some(mut list) = step.as_portal_list().borrow_mut(){
                self.draw_snapshots(cx, &mut *list, scope, root_id)
            }
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let _snapshots = self.view.portal_list(id!(list));
        self.view.handle_event(cx, event, scope);
        let _data = scope.data.get_mut::<AppData>().unwrap();
        if let Event::Actions(actions) = event{
            if let Some(_search) = self.view.text_input(id!(search_input)).changed(&actions){
            }
        }
    }
}