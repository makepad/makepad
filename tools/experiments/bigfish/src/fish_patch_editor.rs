use crate::{
    fish_block_template::FishBlockCategory, fish_doc::FishDoc, fish_ports::ConnectionType,
    makepad_draw::*, makepad_widgets::*,
};

live_design! {
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    import crate::fish_block_editor::*;
    import crate::fish_theme::*;
    import crate::fish_connection_widget::*;

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

        AudioButtonTemplate = <Button>{flow: Overlay, draw_bg: { bodytop: (CABLE_AUDIO_COLOR);}};
        ControlButtonTemplate = <Button>{flow: Overlay, draw_bg: { bodytop:  (CABLE_CONTROL_COLOR);}};
        GateButtonTemplate = <Button>{flow: Overlay, draw_bg: { bodytop: (CABLE_GATE_COLOR);}};
        MIDIButtonTemplate = <Button>{flow: Overlay, draw_bg: { bodytop:(CABLE_MIDI_COLOR);}};

        draw_bg: {
            fn pixel(self) -> vec4 {

                let Pos = floor(self.pos*self.rect_size *0.10);
                let PatternMask = mod(Pos.x + mod(Pos.y, 2.0), 2.0);
                return mix( vec4(0,0.15*self.pos.y,0.1,1), vec4(.05, 0.03, .23*self.pos.y, 1.0), PatternMask);
            }
        }
    }
}

#[derive(Live, LiveRegisterWidget, WidgetRef, WidgetSet)]
pub struct FishPatchEditor {
    #[animator]
    animator: Animator,
    #[walk]
    walk: Walk,
    #[live]
    draw_ls: DrawLine,

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
}

impl Widget for FishPatchEditor {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut WidgetScope,
    ) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);

        self.scroll_bars.handle_event(cx, event);

        for (item_id, item) in self.items.values_mut() {
            let item_uid = item.widget_uid();
            scope.with_id(*item_id, |scope| {
                cx.group_widget_actions(uid, item_uid, |cx|{
                    item.handle_event(cx, event, scope);
                })
            })
        }
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx)
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut WidgetScope, walk: Walk) -> WidgetDraw {
        let patch = &mut scope.data.get_mut::<FishDoc>().patches[0];
        //let mut _fullrect = cx.walk_turtle_with_area(&mut self.area, walk);

        self.scroll_bars.begin(cx, walk, Layout::flow_overlay());

        let _turtle_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();
        self.unscrolled_rect = cx.turtle().unscrolled_rect();
        self.draw_bg.draw_abs(cx, cx.turtle().unscrolled_rect());

        for i in &mut patch.blocks.iter() {
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

            let item = self.item(cx, item_id, templateid).unwrap();

            item.apply_over(
                cx,
                live! {
                    abs_pos: (dvec2(i.x as f64, i.y as f64 )-scroll_pos) ,
                },
            );
            //println!("{} {:?} ({:?},{:?})", item_id, i.id, i.x, i.y);

            item.draw_all(cx, &mut WidgetScope::default());
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

                        abs_pos: (dvec2(i.x as f64, i.y as f64 + inp.id as f64 * 20.)-scroll_pos) ,
                    },
                );
                item.draw_all(cx, &mut WidgetScope::default());
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
                        abs_pos: (dvec2(i.x as f64 + 240., i.y as f64 + outp.id as f64 * 20.)-scroll_pos) ,
                    },
                );
                item.draw_all(cx, &mut WidgetScope::default());
            }
        }

        for i in patch.connections.iter() {
            let item_id = LiveId::from_num(2, i.id as u64);

            let templateid = live_id!(ConnectorTemplate);
            let preitem = self.item(cx, item_id, templateid);
            let item = preitem.unwrap();

            let blockfrom = patch.get_block(i.from_block).unwrap();
            let blockto = patch.get_block(i.to_block).unwrap();
            let _portfrom = blockfrom.get_output_instance(i.from_port).unwrap();
            let _portto = blockto.get_input_instance(i.to_port).unwrap();

            item.apply_over(
                cx,
                live! {
                    start_pos: (dvec2(blockfrom.x as f64 + 250.0, blockfrom.y as f64 + 30.  * _portfrom.id as f64) - scroll_pos),
                    end_pos: (dvec2(blockto.x as f64, blockto.y as f64) - scroll_pos + 30. * _portto.id as f64),
                    color: #ff0,
                      abs_pos: (dvec2(0.,0.)),
                   },
            );

            item.draw_all(cx, &mut WidgetScope::default());

            // println!("{:?} ({:?},{:?})", i.id, i.x,i.y);
        }

        self.scroll_bars.end(cx);

        WidgetDraw::done()
    }
}

impl LiveHook for FishPatchEditor {
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {}

    fn before_apply(&mut self, _cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc { .. } = from {
            self.templates.clear();
        }
    }

    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(
        &mut self,
        cx: &mut Cx,
        from: ApplyFrom,
        index: usize,
        nodes: &[LiveNode],
    ) -> usize {
        let id = nodes[index].id;
        match from {
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
                            node.apply(cx, from, index, nodes);
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
