use crate::{
    block_connector_button::BlockConnectorButtonAction,
    block_delete_button::BlockDeleteButtonAction, block_header_button::BlockHeaderButtonAction,
    fish_block_template::FishBlockCategory, fish_doc::FishDoc, fish_patch::*,
    fish_ports::ConnectionType, makepad_draw::*, makepad_widgets::*,
};

live_design! {
    use makepad_widgets::theme_desktop_dark::*;
    use makepad_widgets::base::*;
    use crate::fish_block_editor::*;
    use crate::fish_theme::*;
    use crate::fish_connection_widget::*;
    use crate::fish_selector_widget::*;
    use crate::block_connector_button::*;

    FishPatchEditor = {{FishPatchEditor}} {
        width: Fill,
        height: Fill,
        scroll_bars: <ScrollBars> {}
        BlockTemplateGenerator = <FishBlockEditorGenerator>{};
        BlockTemplateMeta = <FishBlockEditorMeta>{};
        BlockTemplateFilter = <FishBlockEditorFilter>{};
        BlockTemplateEffect = <FishBlockEditorEffect>{};
        BlockTemplateModulator = <FishBlockEditorModulator>{};
        BlockTemplateEnvelope = <FishBlockEditorEnvelope>{};
        BlockTemplateUtility = <FishBlockEditorUtility>{};

        ConnectorTemplate = <FishConnectionWidget>{color:  #d0d0a0ff};
        SelectorTemplate = <FishSelectorWidget>{color:  #d0d0a0ff};

        AudioButtonTemplate = <BlockConnectorButton>{flow: Overlay, draw_bg: { bodytop: (CABLE_AUDIO_COLOR);}};
        ControlButtonTemplate = <BlockConnectorButton>{flow: Overlay, draw_bg: { bodytop:  (CABLE_CONTROL_COLOR);}};
        GateButtonTemplate = <BlockConnectorButton>{flow: Overlay, draw_bg: { bodytop: (CABLE_GATE_COLOR);}};
        MIDIButtonTemplate = <BlockConnectorButton>{flow: Overlay, draw_bg: { bodytop:(CABLE_MIDI_COLOR);}};

        draw_bg: {
            fn pixel(self) -> vec4 {

              
                return vec4(0.03,0.03,0.03,1);
            }
        }
    }
}

#[derive(Live, Widget)]
pub struct FishPatchEditor {
    #[animator]
    animator: Animator,
    #[walk]
    walk: Walk,
    #[live]
    draw_ls: DrawLine,

    #[redraw]
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    draw_bg: DrawColor,
    #[rust]
    unscrolled_rect: Rect,

    #[rust]
    templates: ComponentMap<LiveId, LivePtr>,
    #[rust]
    items: ComponentMap<LiveId, (LiveId, WidgetRef)>,
    #[rust]
    selectstart: DVec2,
    #[rust]
    selectend: DVec2,
    #[rust]
    selecting: bool,
    #[rust]
    dragstartx: f64,
    #[rust]
    dragstarty: f64,
    #[rust]
    active_undo_level: usize,

    #[rust]
    connectingid: u64,
    #[rust]
    connectingx: f64,
    #[rust]
    connectingy: f64,
    #[rust]
    connectingcurrentx: f64,
    #[rust]
    connectingcurrenty: f64,
    #[rust]
    connectinginput: bool,
    #[rust]
    connecting: bool,

    #[rust]
    selection: FishPatchSelection
    
}

impl Widget for FishPatchEditor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        self.scroll_bars.handle_event(cx, event);
        
        
        for (_item_id, item) in self.items.values_mut() {
            let _item_uid = item.widget_uid();
        
            for action in cx.capture_actions(|cx| item.handle_event(cx, event, scope)) {
                
                match action.as_widget_action().cast() {
                    BlockHeaderButtonAction::Select { id } => {
                        self.selection.clear();
                        self.selection.add(id);
                    }
                    BlockHeaderButtonAction::Move { id, dx, dy } => {
                        self.scroll_bars.redraw(cx);
                        let patch = &mut scope.data.get_mut::<FishDoc>().unwrap().patches[0];
                        patch.move_selection(&self.selection, dx,dy);
                    }
                    BlockHeaderButtonAction::RecordDragStart { id } => {
                        let patch = &mut scope.data.get_mut::<FishDoc>().unwrap().patches[0];
                        let block = patch.blocks.find(id);
                        
                        if block.is_some() {

                            if !self.selection.blocks.contains(&id)
                            {
                                if self.selection.blocks.len() > 0
                                {
                                    self.selection.clear();
                                }
                                self.selection.add(id);
                            }
                          

                            let b = block.unwrap();
                            self.dragstartx = b.x;
                            self.dragstarty = b.y;
                            self.scroll_bars.redraw(cx);
                            self.active_undo_level = patch.undo_checkpoint_start();
                        }
                    }
                    BlockHeaderButtonAction::RecordDragEnd { id: _ } => {
                        let patch = &mut scope.data.get_mut::<FishDoc>().unwrap().patches[0];


                        patch.undo_checkpoint_end_if_match(self.active_undo_level);
                        self.active_undo_level = 0;
                    }
                    _ => {}
                }
                match action.as_widget_action().cast() {
                    BlockDeleteButtonAction::KillBlock { id } => {
                        let patch = &mut scope.data.get_mut::<FishDoc>().unwrap().patches[0];
                        patch.remove_block(id);
                        self.scroll_bars.redraw(cx);
                    }
                    _ => {}
                }
                match action.as_widget_action().cast() {
                    BlockConnectorButtonAction::ConnectStart {
                        id,
                        x,
                        y,
                        frominput,
                    } => {
                        let scroll = self.scroll_bars.get_scroll_pos();

                        self.connectingid = id;
                        self.connectingx = x + scroll.x;
                        self.connectingy = y  + scroll.y ;
                        self.connectingcurrentx = x + scroll.x ;
                        self.connectingcurrenty = y  + scroll.y ;
                        self.connectinginput = frominput;
                        self.connecting = true;
                        self.scroll_bars.redraw(cx);
                    }
                    BlockConnectorButtonAction::Move { id: _, x, y } => {
                        let scroll = self.scroll_bars.get_scroll_pos();

                        self.scroll_bars.redraw(cx);
                        self.connectingcurrentx = x + scroll.x ;
                        self.connectingcurrenty = y  + scroll.y ;
                    }
                    BlockConnectorButtonAction::Released => {
                        self.scroll_bars.redraw(cx);
                        self.connecting = false;
                    }
                    _ => {}
                }
            }
        }



        match event.hits(cx, self.draw_bg.area()) {
            
            Hit::FingerDown(fe) => {
//                if  fe.digit_id == live_id!(0).into() {
                    self.selecting = true;              
                    let patch = &mut scope.data.get_mut::<FishDoc>().unwrap().patches[0];
                            
                    // start selecting
                    self.selectstart = fe.abs.clone();
                    self.selectend = fe.abs.clone();
                    self.selection = patch.select_rectangle(self.selectstart, self.selectend);
  //              }
                self.scroll_bars.redraw(cx);
                self.animator_play(cx, id!(hover.pressed));
            }
            
            Hit::FingerMove(fe) =>{
                if 
                //fe.digit_id == live_id!(0).into() &&
                 self.selecting
                 {
                    self.selectend = fe.abs.clone();
                    let patch = &mut scope.data.get_mut::<FishDoc>().unwrap().patches[0];                   
                    self.selection = patch.select_rectangle(self.selectstart, self.selectend);                    
                    self.scroll_bars.redraw(cx);
                }
                
            }
            Hit::FingerHoverIn(_) => {

                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {

                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => {

                if 
                //fe.digit_id == live_id!(0).into() &&
                 self.selecting {
                    self.selecting = false;                    
                    // stop selecting
                    self.scroll_bars.redraw(cx);
                         }


                if fe.is_over {
                    cx.widget_action(uid, &scope.path, ButtonAction::Clicked);
                    cx.widget_action(uid, &scope.path, ButtonAction::Released);
                    if fe.device.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    } else {
                        self.animator_play(cx, id!(hover.off));
                    }
                } else {
                    cx.widget_action(uid, &scope.path, ButtonAction::Released);
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => (),
        }


    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let patch = &mut scope.data.get_mut::<FishDoc>().unwrap().patches[0];
        //let mut _fullrect = cx.walk_turtle_with_area(&mut self.area, walk);

        self.scroll_bars.begin(cx, walk, Layout::flow_overlay());

        let _turtle_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();
        self.unscrolled_rect = cx.turtle().unscrolled_rect();
        self.draw_bg.draw_abs(cx, cx.turtle().unscrolled_rect());

        self.draw_connections(cx, patch, scroll_pos);
        self.draw_selection(cx,patch, scroll_pos);

        for i in &mut patch.blocks.iter_mut() {
            let item_id = LiveId::from_num(1, i.id as u64);
            let templateid = match i.category {
                FishBlockCategory::Effect => live_id!(BlockTemplateEffect),
                FishBlockCategory::Generator => live_id!(BlockTemplateGenerator),
                FishBlockCategory::Modulator => live_id!(BlockTemplateModulator),
                FishBlockCategory::Envelope => live_id!(BlockTemplateEnvelope),
                FishBlockCategory::Filter => live_id!(BlockTemplateFilter),
                FishBlockCategory::Meta => live_id!(BlockTemplateMeta),
                FishBlockCategory::Utility => live_id!(BlockTemplateUtility),
            };

            let item = self.item(cx, item_id, templateid).unwrap().as_view();

            item.apply_over(
                cx,
                live! {title= {topbar = {header= {text: (i.name) , blockid: (i.id)}, delete = {blockid: (i.id)}}},
                abs_pos: (dvec2(i.x as f64, i.y as f64 )-scroll_pos)},
            );

            item.draw_all(cx, &mut Scope::empty());
            let itemarea = item.area().rect(cx);
            i.h = itemarea.size.y;
            i.w = itemarea.size.x;

            if self.selection.blocks.contains(&i.id){
                self.draw_selected_outline(cx,i.x,i.y, i.w,i.h);
            }

            for inp in &i.input_ports {
                let item_id = LiveId::from_num(2000 + i.id, inp.id as u64);
                let templateid = match inp.datatype {
                    ConnectionType::Audio => live_id!(AudioButtonTemplate),
                    ConnectionType::MIDI => live_id!(MIDIButtonTemplate),
                    ConnectionType::Control => live_id!(ControlButtonTemplate),
                    ConnectionType::Gate => live_id!(GateButtonTemplate),
                    _ => live_id!(AudioButtonTemplate),
                };
                let item = self.item(cx, item_id, templateid).unwrap();
                item.apply_over(
                    cx,
                    live! {
                        input: true,

                        abs_pos: (dvec2(i.x as f64, i.y as f64 + inp.id as f64 * 20.)-scroll_pos) ,
                    },
                );
                item.draw_all(cx, &mut Scope::empty());
            }

            for outp in &i.output_ports {
                let item_id = LiveId::from_num(3000 + i.id, outp.id as u64);
                let templateid = match outp.datatype {
                    ConnectionType::Audio => live_id!(AudioButtonTemplate),
                    ConnectionType::MIDI => live_id!(MIDIButtonTemplate),
                    ConnectionType::Control => live_id!(ControlButtonTemplate),
                    ConnectionType::Gate => live_id!(GateButtonTemplate),
                    _ => live_id!(AudioButtonTemplate),
                };
                let item = self.item(cx, item_id, templateid).unwrap();
                item.apply_over(
                    cx,
                    live! {
                        input: false,
                        abs_pos: (dvec2(i.x as f64 + itemarea.size.x  - 20. + 1., i.y as f64 + outp.id as f64 * 20.)-scroll_pos) ,
                    },
                );
                item.draw_all(cx, &mut Scope::empty());
            }
        }

        if self.connecting {
            self.draw_active_connection(cx, patch, scroll_pos);
        }
        self.scroll_bars.end(cx);

        DrawStep::done()
    }
}

impl LiveHook for FishPatchEditor {
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {}

    fn before_apply(&mut self, _cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc { .. } = apply.from {
            self.templates.clear();
        }
    }

    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(
        &mut self,
        cx: &mut Cx,
        apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> usize {
        let id = nodes[index].id;
        match apply.from {
            ApplyFrom::NewFromDoc { file_id } | ApplyFrom::UpdateFromDoc { file_id } => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    let live_ptr = cx
                        .live_registry
                        .borrow()
                        .file_id_index_to_live_ptr(file_id, index);
                    self.templates.insert(id, live_ptr);
                    // lets apply this thing over all our childnodes with that template
                    for (templ_id, node) in self.items.values_mut() {
                        if *templ_id == id {
                            node.apply(cx, apply, index, nodes);
                        }
                    }
                } else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                }
            }
            _ => (),
        }
        nodes.skip_node(index)
    }
}

impl FishPatchEditor {
    pub fn item(&mut self, cx: &mut Cx, id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template) {
            let (_, entry) = self.items.get_or_insert(cx, id, |cx| {
                (template, WidgetRef::new_from_ptr(cx, Some(*ptr)))
            });
            return Some(entry.clone());
        }
        None
    }
}

impl FishPatchEditor {
    pub fn draw_active_connection(&mut self, cx: &mut Cx2d, _patch: &FishPatch, scroll_pos: DVec2) {
        let item_id = LiveId::from_str("ActiveConnectionWidget");

        let templateid = live_id!(ConnectorTemplate);
        let preitem = self.item(cx, item_id, templateid);
        let item = preitem.unwrap();
        if self.connectinginput {
            item.apply_over(
                cx,
                live! {
                    end_pos: (dvec2(self.connectingx as f64 , self.connectingy as f64 ) - scroll_pos),
                    start_pos: (dvec2(self.connectingcurrentx as f64, self.connectingcurrenty as f64) - scroll_pos ),
                    color: #ff0,
                      abs_pos: (dvec2(0.,0.)),
                   },
            );
        } else {
            item.apply_over(
            cx,
            live! {
                start_pos: (dvec2(self.connectingx as f64 , self.connectingy as f64 ) - scroll_pos),
                end_pos: (dvec2(self.connectingcurrentx as f64, self.connectingcurrenty as f64) - scroll_pos ),
                color: #ff0,
                  abs_pos: (dvec2(0.,0.)),
               },
        );
        }
        item.draw_all(cx, &mut Scope::empty());
    }

    pub fn draw_selection(&mut self, cx: &mut Cx2d, _patch: &FishPatch, scroll_pos: DVec2) {
        if self.selecting
        {
            let item_id = LiveId::from_str("ActiveSelectionWidget");
            let templateid = live_id!(SelectorTemplate);
            let preitem = self.item(cx, item_id, templateid);
            let item = preitem.unwrap();
            item.apply_over(cx, live! {
                                start_pos: (self.selectstart - scroll_pos),
                              end_pos: (self.selectend - scroll_pos),
                              color: #eee,
                              line_width: 2.0
                            });
            item.draw_all(cx, &mut Scope::empty());
        }
    }

    pub fn draw_selected_outline(&mut self, cx: &mut Cx2d, x: f64, y:f64, w: f64, h: f64){

        let tl = DVec2{x: x as f64 - 1.5,y: y as f64  - 1.5};
        let br = DVec2{x: (x+w ) as f64+1.5,y: (y+h) as f64+1.5};

        let bl = DVec2{x: tl.x, y: br.y} ;
        let tr = DVec2{x: br.x, y: tl.y} ;
        let color =         vec4(1.,1.,0.,1.);
        self.draw_ls.draw_line_abs(cx, tl, tr, color, 4.);
        self.draw_ls.draw_line_abs(cx, tr, br, color, 4.);
        self.draw_ls.draw_line_abs(cx, br, bl, color, 4.);
        self.draw_ls.draw_line_abs(cx, tl, bl, color, 4.);
    }

    pub fn draw_connections(&mut self, cx: &mut Cx2d, patch: &FishPatch, scroll_pos: DVec2) {
        for i in patch.connections.iter() {
            let item_id = LiveId::from_num(2, i.id as u64);

            let templateid = live_id!(ConnectorTemplate);
            let preitem = self.item(cx, item_id, templateid);
            let item = preitem.unwrap();

            let blockfromopt = patch.get_block(&i.from_block);
            let blocktoopt = patch.get_block(&i.to_block);
            let mut inselection = false;

            if blockfromopt.is_some() && blocktoopt.is_some() {
                let blockfrom = blockfromopt.unwrap();
                let blockto = blocktoopt.unwrap();
                let _portfrom = blockfrom.get_output_instance(i.from_port).unwrap();
                let _portto = blockto.get_input_instance(i.to_port).unwrap();
                
                if self.selection.blocks.contains(&blockfrom.id) && self.selection.blocks.contains(&blockto.id) {
                    inselection = true;
                }

                item.apply_over( cx, live! {
                    start_pos: (dvec2(blockfrom.x as f64 + 200.0, blockfrom.y as f64 + 10. + 20.  * _portfrom.id as f64) - scroll_pos),
                    end_pos: (dvec2(blockto.x as f64, blockto.y as f64+ 10. + 20. * _portto.id as f64) - scroll_pos ),
                    from_top: (blockfrom.y- scroll_pos.y ),
                    from_bottom: (blockfrom.y + blockfrom.h - scroll_pos.y ),
                    to_top: (blockto.y - scroll_pos.y ),
                    to_bottom: (blockto.y + blockto.h - scroll_pos.y ) ,
                    color: #x888,
                      abs_pos: (dvec2(0.,0.)),
                      selected: (inselection)
                   },
            );

                item.draw_all(cx, &mut Scope::empty());
            }
            // println!("{:?} ({:?},{:?})", i.id, i.x,i.y);
        }
    }
}
