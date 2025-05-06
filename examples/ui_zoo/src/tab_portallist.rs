use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    IMG_A = dep("crate://self/resources/ducky.png")
    IMG_PROFILE_A = dep("crate://self/resources/ducky.png")
    COLOR_TEXT = #x8
    COLOR_TEXT_LIGHT = #xCCC
    COLOR_USER = #x444

    Post = <View> {
        width: Fill, height: Fit,
        padding: { top: 10., bottom: 10.}

        body = <RoundedView> {
            width: Fill, height: Fit
            content = <View> {
                width: Fill, height: Fit
                text = <P> { text: "" }
            }
        }
    }

    NewsFeed = {{NewsFeed}} {
        list = <PortalList> {
            scroll_bar: <ScrollBar> {}
            TopSpace = <View> {height: 0.}
            BottomSpace = <View> {height: 100.}

            Post = <CachedView>{
                flow: Down,
                <Post> {}
                <Hr> {}
            }
        }
    }

    pub DemoPortalList = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/portallist.md") } 
        }
        demos = {
            news_feed = <NewsFeed> {}
        }
    }
}
#[derive(Live, LiveHook, Widget)]
struct NewsFeed{
    #[deref] view:View,
}

impl Widget for NewsFeed{
    fn draw_walk(&mut self, cx:&mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while let Some(item) =  self.view.draw_walk(cx, scope, walk).step(){
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, 1000);
                while let Some(item_id) = list.next_visible_item(cx) {
                    let template = match item_id{
                        0 => live_id!(TopSpace),
                        _ => live_id!(Post)
                    };
                    let item = list.item(cx, item_id, template);
                    let text = match item_id % 4 {
                        1 => format!("At vero eos et accusam et justo duo dolores et ea rebum. Stet clita kasd gubergren, no sea takimata sanctus est Lorem ipsum dolor sit amet. Lorem ipsum dolor sit amet, consetetur sadipscing elitr, sed diam nonumy eirmod tempor invidunt ut labore et dolore magna aliquyam erat, sed diam voluptua. At vero eos et accusam et justo duo dolores et ea rebum."),
                        2 => format!("How are you?"),
                        3 => format!("Stet clita kasd gubergren, no sea takimata sanctus est Lorem ipsum dolor sit amet."),
                        _ => format!("Lorem ipsum dolor sit amet, consetetur sadipscing elitr, sed diam nonumy eirmod tempor invidunt ut labore et dolore magna aliquyam erat, sed diam voluptua."),
                    };
                    item.label(id!(content.text)).set_text(cx, &text);
                    item.button(id!(likes)).set_text(cx, &format!("{}", item_id % 23));
                    item.button(id!(comments)).set_text(cx, &format!("{}", item_id % 6));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
    fn handle_event(&mut self, cx:&mut Cx, event:&Event, scope:&mut Scope){
        self.view.handle_event(cx, event, scope)
    }
}