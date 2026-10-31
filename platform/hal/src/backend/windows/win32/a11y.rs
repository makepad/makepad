//! UI Automation maps the semantic mirror and the shared action path. COM callbacks never
//! enter the window shell or hold a mirror/cache lock across a Windows call. Notifications
//! are captured before mirror replacement and raised on the shell thread after HAL submit.
use super::lock;
use crate::a11y::{inject_action, request_consumer, set_os_selfcheck, wait_done, with_tree};
use makepad_a11y::{
    consumer,
    mirror::{text, Tree},
    ActionArg, ActionRequest, ActionResult, Actions, Live, Node, NodeId, Notification, Rect, Role,
    States, TextInfo, Toggle, TreeUpdate,
};
use makepad_windows_sys::{
    core::{ObjectBuilder, SafeArray, Variant, BOOL, BSTR, HRESULT, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::ClientToScreen,
        System::{Com::*, Ole::*, Variant::*},
        UI::{Accessibility::*, HiDpi::GetDpiForWindow},
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

type Result<T> = std::result::Result<T, HRESULT>;
const GONE: HRESULT = HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32);
const UNSUPPORTED: HRESULT = HRESULT(UIA_E_NOTSUPPORTED as i32);
const DISABLED: HRESULT = HRESULT(UIA_E_ELEMENTNOTENABLED as i32);

/// HWND is an opaque process handle, not a pointer that we dereference. The root uses NONE
/// so the first WM_GETOBJECT can return it before the first semantic frame arrives.
#[derive(Clone, Copy)]
struct Provider {
    key: u64,
    id: NodeId,
    hwnd: usize,
}
struct Cached {
    simple: IRawElementProviderSimple,
    patterns: u32,
}
// SAFETY: only our ObjectBuilder objects are stored here. Every slot is Send + Sync;
// their atomic COM references and mirror-backed methods support arbitrary UIA threads.
unsafe impl Send for Cached {}
unsafe impl Sync for Cached {}
struct Window {
    hwnd: usize,
    providers: HashMap<NodeId, Arc<Cached>>,
}
fn windows() -> &'static Mutex<HashMap<u64, Window>> {
    static WINDOWS: OnceLock<Mutex<HashMap<u64, Window>>> = OnceLock::new();
    WINDOWS.get_or_init(Default::default)
}

pub(super) fn register(key: u64, hwnd: HWND) {
    set_os_selfcheck(selfcheck);
    let old = {
        let mut windows = lock(windows());
        if windows.get(&key).is_some_and(|w| w.hwnd == hwnd.0 as usize) {
            return;
        }
        windows.insert(
            key,
            Window {
                hwnd: hwnd.0 as usize,
                providers: HashMap::new(),
            },
        )
    };
    if let Some(old) = old {
        disconnect(old.providers.into_values());
    }
}
pub(super) fn unregister(key: u64) {
    let old = lock(windows()).remove(&key);
    if let Some(old) = old {
        disconnect(old.providers.into_values());
    }
}
pub(super) fn close_session(session: u32) {
    let keys: Vec<_> = lock(windows())
        .keys()
        .filter(|key| (**key >> 32) as u32 == session)
        .copied()
        .collect();
    for key in keys {
        unregister(key);
    }
}
fn disconnect(providers: impl IntoIterator<Item = Arc<Cached>>) {
    for provider in providers {
        unsafe {
            let _ = UiaDisconnectProvider(&provider.simple);
        }
    }
}
pub(super) fn get_object(key: u64, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if crate::a11y::consumers() & consumer::OS == 0 {
        request_consumer(consumer::OS, true);
    }
    // WM_GETOBJECT can arrive during CreateWindowEx, before register().
    if !lock(windows()).contains_key(&key) {
        register(key, hwnd);
    }
    match provider(key, NodeId::NONE) {
        Ok(p) => unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &p) },
        Err(_) => LRESULT(0),
    }
}

const INVOKE: u32 = 1;
const TOGGLE: u32 = 2;
const VALUE: u32 = 4;
const RANGE: u32 = 8;
const SELECT_ITEM: u32 = 16;
const SELECT: u32 = 32;
const EXPAND: u32 = 64;
const SCROLL: u32 = 128;
const SCROLL_ITEM: u32 = 256;
const TEXT: u32 = 512;
const GRID: u32 = 1024;
const GRID_ITEM: u32 = 2048;
const TABLE: u32 = 4096;
const TABLE_ITEM: u32 = 8192;
const PATTERNS: &[(UIA_PATTERN_ID, u32, &str)] = &[
    (UIA_InvokePatternId, INVOKE, "invoke"),
    (UIA_TogglePatternId, TOGGLE, "toggle"),
    (UIA_ValuePatternId, VALUE, "value"),
    (UIA_RangeValuePatternId, RANGE, "range"),
    (UIA_SelectionItemPatternId, SELECT_ITEM, "selection_item"),
    (UIA_SelectionPatternId, SELECT, "selection"),
    (UIA_ExpandCollapsePatternId, EXPAND, "expand"),
    (UIA_ScrollPatternId, SCROLL, "scroll"),
    (UIA_ScrollItemPatternId, SCROLL_ITEM, "scroll_item"),
    (UIA_TextPatternId, TEXT, "text"),
    (UIA_TextPattern2Id, TEXT, "text2"),
    (UIA_GridPatternId, GRID, "grid"),
    (UIA_GridItemPatternId, GRID_ITEM, "grid_item"),
    (UIA_TablePatternId, TABLE, "table"),
    (UIA_TableItemPatternId, TABLE_ITEM, "table_item"),
];
fn patterns(t: &Tree, n: &Node) -> u32 {
    let mut bits = 0;
    for (on, bit) in [
        (n.actions.contains(Actions::PRESS), INVOKE),
        (n.toggle != Toggle::None, TOGGLE),
        (
            n.states.contains(States::EDITABLE) || !n.value.is_empty(),
            VALUE,
        ),
        (n.range.is_some(), RANGE),
        (n.states.contains(States::SELECTABLE), SELECT_ITEM),
        (
            n.states.contains(States::MULTISELECTABLE)
                || t.children(n.id)
                    .any(|c| c.states.contains(States::SELECTABLE)),
            SELECT,
        ),
        (
            n.states.intersects(States::EXPANDED | States::COLLAPSED)
                || n.actions.intersects(Actions::EXPAND | Actions::COLLAPSE),
            EXPAND,
        ),
        (n.actions.contains(Actions::SCROLL), SCROLL),
        (n.actions.contains(Actions::SCROLL_INTO_VIEW), SCROLL_ITEM),
        (n.text.is_some(), TEXT),
        (
            n.table.is_some() && matches!(n.role, Role::Grid | Role::Table),
            GRID,
        ),
        (
            n.table.is_some() && !matches!(n.role, Role::Grid | Role::Table | Role::Row),
            GRID_ITEM,
        ),
        (n.table.is_some() && n.role == Role::Table, TABLE),
        (
            n.table.is_some()
                && matches!(n.role, Role::Cell | Role::RowHeader | Role::ColumnHeader),
            TABLE_ITEM,
        ),
    ] {
        if on {
            bits |= bit;
        }
    }
    bits
}
fn provider(key: u64, id: NodeId) -> Result<IRawElementProviderSimple> {
    let (id, bits) = with_tree(key, |t| {
        let id = if id == t.root { NodeId::NONE } else { id };
        let n = t.node(if id.is_none() { t.root } else { id });
        (id, n.map(|n| patterns(t, n)))
    })
    .unwrap_or((id, None));
    if bits.is_none() && !id.is_none() {
        return Err(GONE);
    }
    let bits = bits.unwrap_or(0);
    let hwnd = {
        let windows = lock(windows());
        let window = windows.get(&key).ok_or(GONE)?;
        if let Some(p) = window.providers.get(&id).filter(|p| p.patterns == bits) {
            return Ok(p.simple.clone());
        }
        window.hwnd
    };
    let p = Provider { key, id, hwnd };
    let mut b = ObjectBuilder::new()
        .add_IRawElementProviderSimple(Box::new(p))
        .add_IRawElementProviderFragment(Box::new(p));
    if id.is_none() {
        b = b.add_IRawElementProviderFragmentRoot(Box::new(p));
    }
    if bits & INVOKE != 0 {
        b = b.add_IInvokeProvider(Box::new(p));
    }
    if bits & TOGGLE != 0 {
        b = b.add_IToggleProvider(Box::new(p));
    }
    if bits & VALUE != 0 {
        b = b.add_IValueProvider(Box::new(p));
    }
    if bits & RANGE != 0 {
        b = b.add_IRangeValueProvider(Box::new(p));
    }
    if bits & SELECT_ITEM != 0 {
        b = b.add_ISelectionItemProvider(Box::new(p));
    }
    if bits & SELECT != 0 {
        b = b.add_ISelectionProvider(Box::new(p));
    }
    if bits & EXPAND != 0 {
        b = b.add_IExpandCollapseProvider(Box::new(p));
    }
    if bits & SCROLL != 0 {
        b = b.add_IScrollProvider(Box::new(p));
    }
    if bits & SCROLL_ITEM != 0 {
        b = b.add_IScrollItemProvider(Box::new(p));
    }
    if bits & TEXT != 0 {
        b = b.add_ITextProvider2(Box::new(p));
    }
    if bits & GRID != 0 {
        b = b.add_IGridProvider(Box::new(p));
    }
    if bits & GRID_ITEM != 0 {
        b = b.add_IGridItemProvider(Box::new(p));
    }
    if bits & TABLE != 0 {
        b = b.add_ITableProvider(Box::new(p));
    }
    if bits & TABLE_ITEM != 0 {
        b = b.add_ITableItemProvider(Box::new(p));
    }
    let new = Arc::new(Cached {
        simple: unsafe { IRawElementProviderSimple::from_raw(b.finish()?)? },
        patterns: bits,
    });
    let (result, old) = {
        let mut windows = lock(windows());
        let window = windows
            .get_mut(&key)
            .filter(|w| w.hwnd == hwnd)
            .ok_or(GONE)?;
        if let Some(p) = window.providers.get(&id).filter(|p| p.patterns == bits) {
            (p.simple.clone(), None)
        } else {
            let old = window.providers.insert(id, new.clone());
            (new.simple.clone(), old)
        }
    };
    disconnect(old);
    Ok(result)
}
impl Provider {
    fn read<T>(&self, f: impl FnOnce(&Tree, &Node) -> T) -> Result<T> {
        if !lock(windows())
            .get(&self.key)
            .is_some_and(|w| w.hwnd == self.hwnd)
        {
            return Err(GONE);
        }
        with_tree(self.key, |t| {
            t.node(if self.id.is_none() { t.root } else { self.id })
                .map(|n| f(t, n))
        })
        .flatten()
        .ok_or(GONE)
    }
    fn simple(&self) -> Result<IRawElementProviderSimple> {
        self.read(|_, _| ())?;
        provider(self.key, self.id)
    }
    fn fragment(&self, id: Option<NodeId>) -> Result<Option<IRawElementProviderFragment>> {
        id.map(|id| unsafe { IRawElementProviderFragment::query(provider(self.key, id)?.as_raw()) })
            .transpose()
    }
    fn action(&self, action: Actions, arg: ActionArg) -> Result<()> {
        let (id, states, actions) = self.read(|_, n| (n.id, n.states, n.actions))?;
        if states.contains(States::DISABLED) {
            return Err(DISABLED);
        }
        if !actions.contains(action) {
            return Err(UNSUPPORTED);
        }
        let req = inject_action(
            self.key,
            ActionRequest {
                req: 0,
                node: id,
                action,
                arg,
            },
        );
        let observed = {
            let mut watch = lock(action_watch());
            if let Some(w) = watch.as_mut().filter(|w| {
                w.key == self.key && w.node == id && action == Actions::PRESS && w.req.is_none()
            }) {
                w.req = Some(req);
                action_signal().notify_all();
                true
            } else {
                false
            }
        };
        if !observed {
            crate::a11y::forget_action(req);
        }
        Ok(())
    }
    fn array(&self, ids: Vec<NodeId>) -> Result<*mut SAFEARRAY> {
        let items = ids
            .into_iter()
            .map(|id| unsafe { IUnknown::query(provider(self.key, id)?.as_raw()) })
            .collect::<Result<Vec<_>>>()?;
        Ok(SafeArray::from_unknowns(&items)?.into_raw())
    }
    fn ancestor(&self, predicate: impl Fn(&Node) -> bool) -> Result<NodeId> {
        self.read(|t, n| {
            t.ancestors(n.id)
                .into_iter()
                .rev()
                .find(|id| t.node(*id).is_some_and(&predicate))
        })?
        .ok_or(UNSUPPORTED)
    }
    fn table(&self) -> Result<makepad_a11y::TableInfo> {
        self.read(|_, n| n.table)?.ok_or(UNSUPPORTED)
    }
    fn text(&self) -> Result<TextInfo> {
        self.read(|_, n| n.text.clone())?.ok_or(UNSUPPORTED)
    }
}
fn transform(hwnd: usize) -> (f64, POINT) {
    let mut origin = POINT::default();
    unsafe {
        let _ = ClientToScreen(HWND(hwnd as _), &mut origin);
    }
    let scale = unsafe { GetDpiForWindow(HWND(hwnd as _)) } as f64 / 96.0;
    (if scale > 0.0 { scale } else { 1.0 }, origin)
}
fn screen_rect(hwnd: usize, r: Rect) -> UiaRect {
    if r.is_empty() {
        return UiaRect::default();
    }
    let (s, o) = transform(hwnd);
    // UIA quantizes the origin and size independently, even through its R8 property.
    // Enclose the layout rectangle in physical pixels before crossing that boundary.
    let left = (o.x as f64 + r.x as f64 * s).floor();
    let top = (o.y as f64 + r.y as f64 * s).floor();
    let right = (o.x as f64 + (r.x as f64 + r.w as f64) * s).ceil();
    let bottom = (o.y as f64 + (r.y as f64 + r.h as f64) * s).ceil();
    UiaRect {
        left,
        top,
        width: right - left,
        height: bottom - top,
    }
}
fn local_point(hwnd: usize, x: f64, y: f64) -> (f32, f32) {
    let (s, o) = transform(hwnd);
    (((x - o.x as f64) / s) as f32, ((y - o.y as f64) / s) as f32)
}
fn doubles(values: &[f64]) -> Result<SafeArray> {
    let count = u32::try_from(values.len()).map_err(|_| E_INVALIDARG)?;
    let a = unsafe { SafeArray::from_raw(SafeArrayCreateVector(VT_R8, 0, count))? };
    if !values.is_empty() {
        unsafe {
            let data = SafeArrayAccessData(a.as_raw())?;
            std::ptr::copy_nonoverlapping(values.as_ptr(), data.cast::<f64>(), values.len());
            SafeArrayUnaccessData(a.as_raw())?;
        }
    }
    Ok(a)
}
fn rect_variant(r: UiaRect) -> Result<Variant> {
    let mut v = VARIANT::default();
    v.Anonymous.Anonymous.vt = VARENUM(VT_ARRAY.0 | VT_R8.0);
    v.Anonymous.Anonymous.Anonymous.parray =
        doubles(&[r.left, r.top, r.width, r.height])?.into_raw();
    Ok(unsafe { Variant::from_raw(v) })
}
fn bstr(s: &str) -> Result<*mut u16> {
    let b = BSTR::from_str(s)?;
    let p = b.0;
    std::mem::forget(b);
    Ok(p)
}
unsafe fn wide(p: *const u16) -> Result<String> {
    if p.is_null() {
        return Err(E_INVALIDARG);
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    Ok(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
}
fn toggle(t: Toggle) -> ToggleState {
    match t {
        Toggle::On => ToggleState_On,
        Toggle::Mixed => ToggleState_Indeterminate,
        _ => ToggleState_Off,
    }
}
fn expand(s: States) -> ExpandCollapseState {
    if s.contains(States::EXPANDED) {
        ExpandCollapseState_Expanded
    } else if s.contains(States::COLLAPSED) {
        ExpandCollapseState_Collapsed
    } else {
        ExpandCollapseState_LeafNode
    }
}
impl IRawElementProviderSimpleImpl for Provider {
    fn get_ProviderOptions(&self) -> Result<ProviderOptions> {
        // No UseComThreading: UIA may call these synchronized providers on its own threads.
        Ok(ProviderOptions(
            ProviderOptions_ServerSideProvider.0
                | ProviderOptions_ProviderOwnsSetFocus.0
                | ProviderOptions_RefuseNonClientSupport.0,
        ))
    }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<Option<IUnknown>> {
        let bit = PATTERNS
            .iter()
            .find(|(pattern, _, _)| *pattern == id)
            .map_or(0, |(_, bit, _)| *bit);
        if self.read(|t, n| patterns(t, n))? & bit == 0 {
            return Ok(None);
        }
        unsafe { IUnknown::query(self.simple()?.as_raw()).map(Some) }
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        // Copy just the requested field. A geometry/focus query must not clone a document.
        let value = self.read(|_, n| match id {
            v if v == UIA_ControlTypePropertyId => PropertyValue::Int(n.role.uia_control_type()),
            v if v == UIA_NamePropertyId => PropertyValue::Text(n.name.clone()),
            v if v == UIA_HelpTextPropertyId => PropertyValue::Text(n.description.clone()),
            v if v == UIA_AutomationIdPropertyId => PropertyValue::Text(if n.path.is_empty() {
                n.id.short()
            } else {
                n.path.clone()
            }),
            v if v == UIA_ClassNamePropertyId => PropertyValue::Text(n.class.clone()),
            v if v == UIA_LocalizedControlTypePropertyId && !n.role_description.is_empty() => {
                PropertyValue::Text(n.role_description.clone())
            }
            v if v == UIA_FrameworkIdPropertyId => PropertyValue::Text("Makepad".into()),
            v if v == UIA_BoundingRectanglePropertyId => PropertyValue::Bounds(n.bounds),
            v if v == UIA_IsEnabledPropertyId => {
                PropertyValue::Bool(!n.states.contains(States::DISABLED))
            }
            v if v == UIA_HasKeyboardFocusPropertyId => {
                PropertyValue::Bool(n.states.contains(States::FOCUSED))
            }
            v if v == UIA_IsKeyboardFocusablePropertyId => {
                PropertyValue::Bool(n.states.contains(States::FOCUSABLE))
            }
            v if v == UIA_IsOffscreenPropertyId => {
                PropertyValue::Bool(n.states.intersects(States::OFFSCREEN | States::HIDDEN))
            }
            v if v == UIA_IsPasswordPropertyId => {
                PropertyValue::Bool(n.states.contains(States::PROTECTED))
            }
            v if v == UIA_IsControlElementPropertyId || v == UIA_IsContentElementPropertyId => {
                PropertyValue::Bool(!n.states.contains(States::HIDDEN) && n.role != Role::Generic)
            }
            v if v == UIA_LiveSettingPropertyId => PropertyValue::Int(n.live as i32),
            v if v == UIA_PositionInSetPropertyId => PropertyValue::Int(n.pos_in_set as i32),
            v if v == UIA_SizeOfSetPropertyId => PropertyValue::Int(n.set_size as i32),
            v if v == UIA_LevelPropertyId => PropertyValue::Int(n.level as i32),
            _ => PropertyValue::Empty,
        })?;
        Ok(value.variant(self.hwnd)?.into_raw())
    }
    fn get_HostRawElementProvider(&self) -> Result<Option<IRawElementProviderSimple>> {
        if self.id.is_none() {
            unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as _)).map(Some) }
        } else {
            self.read(|_, _| ())?;
            Ok(None)
        }
    }
}
impl IRawElementProviderFragmentImpl for Provider {
    fn Navigate(
        &self,
        direction: NavigateDirection,
    ) -> Result<Option<IRawElementProviderFragment>> {
        let id = self.read(|t, n| match direction {
            v if v == NavigateDirection_Parent => t.parent(n.id),
            v if v == NavigateDirection_FirstChild => t.children(n.id).next().map(|n| n.id),
            v if v == NavigateDirection_LastChild => t.children(n.id).last().map(|n| n.id),
            v if v == NavigateDirection_NextSibling || v == NavigateDirection_PreviousSibling => {
                let p = t.parent(n.id).and_then(|p| t.node(p));
                p.and_then(|p| {
                    p.children.iter().position(|id| *id == n.id).and_then(|i| {
                        let j = if direction == NavigateDirection_NextSibling {
                            i.checked_add(1)
                        } else {
                            i.checked_sub(1)
                        };
                        j.and_then(|j| p.children.get(j)).copied()
                    })
                })
            }
            _ => None,
        })?;
        self.fragment(id)
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        let id = self.read(|_, n| n.id.0)?;
        Ok(SafeArray::from_i32s(&[
            UiaAppendRuntimeId as i32,
            self.key as i32,
            id as i32,
            (id >> 32) as i32,
        ])?
        .into_raw())
    }
    fn get_BoundingRectangle(&self) -> Result<UiaRect> {
        Ok(screen_rect(self.hwnd, self.read(|_, n| n.bounds)?))
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        self.read(|_, _| ())?;
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        self.action(Actions::FOCUS, ActionArg::None)
    }
    fn get_FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        self.read(|_, _| ())?;
        unsafe {
            IRawElementProviderFragmentRoot::query(provider(self.key, NodeId::NONE)?.as_raw())
        }
    }
}
impl IRawElementProviderFragmentRootImpl for Provider {
    fn ElementProviderFromPoint(
        &self,
        x: f64,
        y: f64,
    ) -> Result<Option<IRawElementProviderFragment>> {
        let (x, y) = local_point(self.hwnd, x, y);
        self.fragment(self.read(|t, _| t.hit_test(x, y))?)
    }
    fn GetFocus(&self) -> Result<Option<IRawElementProviderFragment>> {
        self.fragment(self.read(|t, _| (!t.focus.is_none()).then_some(t.focus))?)
    }
}

impl IInvokeProviderImpl for Provider {
    fn Invoke(&self) -> Result<()> {
        self.action(Actions::PRESS, ActionArg::None)
    }
}
impl IToggleProviderImpl for Provider {
    fn Toggle(&self) -> Result<()> {
        self.action(Actions::TOGGLE, ActionArg::None)
    }
    fn get_ToggleState(&self) -> Result<ToggleState> {
        self.read(|_, n| toggle(n.toggle))
    }
}
impl IValueProviderImpl for Provider {
    fn SetValue(&self, value: PCWSTR) -> Result<()> {
        if self.read(|_, n| n.states.contains(States::READ_ONLY))? {
            return Err(UNSUPPORTED);
        }
        self.action(
            Actions::SET_VALUE,
            ActionArg::Text(unsafe { wide(value.0)? }),
        )
    }
    fn get_Value(&self) -> Result<*mut u16> {
        bstr(&self.read(|_, n| n.value.clone())?)
    }
    fn get_IsReadOnly(&self) -> Result<BOOL> {
        self.read(|_, n| {
            BOOL(
                (n.states.contains(States::READ_ONLY) || !n.actions.contains(Actions::SET_VALUE))
                    as i32,
            )
        })
    }
}
impl IRangeValueProviderImpl for Provider {
    fn SetValue(&self, value: f64) -> Result<()> {
        let (range, states) = self.read(|_, n| (n.range, n.states))?;
        let range = range.ok_or(UNSUPPORTED)?;
        if states.contains(States::READ_ONLY) {
            return Err(UNSUPPORTED);
        }
        if !value.is_finite() || value < range.min || value > range.max {
            return Err(E_INVALIDARG);
        }
        self.action(Actions::SET_VALUE, ActionArg::Number(value))
    }
    fn get_Value(&self) -> Result<f64> {
        self.read(|_, n| n.range.map(|r| r.now))?.ok_or(UNSUPPORTED)
    }
    fn get_IsReadOnly(&self) -> Result<BOOL> {
        IValueProviderImpl::get_IsReadOnly(self)
    }
    fn get_Maximum(&self) -> Result<f64> {
        self.read(|_, n| n.range.map(|r| r.max))?.ok_or(UNSUPPORTED)
    }
    fn get_Minimum(&self) -> Result<f64> {
        self.read(|_, n| n.range.map(|r| r.min))?.ok_or(UNSUPPORTED)
    }
    fn get_LargeChange(&self) -> Result<f64> {
        self.read(|_, n| n.range.map(|r| r.step.max((r.max - r.min) / 10.0)))?
            .ok_or(UNSUPPORTED)
    }
    fn get_SmallChange(&self) -> Result<f64> {
        self.read(|_, n| n.range.map(|r| r.step))?
            .ok_or(UNSUPPORTED)
    }
}
impl ISelectionItemProviderImpl for Provider {
    fn Select(&self) -> Result<()> {
        self.action(Actions::SELECT, ActionArg::None)
    }
    fn AddToSelection(&self) -> Result<()> {
        self.action(Actions::ADD_SELECTION, ActionArg::None)
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        self.action(Actions::REMOVE_SELECTION, ActionArg::None)
    }
    fn get_IsSelected(&self) -> Result<BOOL> {
        self.read(|_, n| BOOL(n.states.contains(States::SELECTED) as i32))
    }
    fn get_SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        let id = self
            .read(|t, n| {
                t.ancestors(n.id)
                    .into_iter()
                    .rev()
                    .find(|id| t.node(*id).is_some_and(|n| patterns(t, n) & SELECT != 0))
            })?
            .ok_or(UNSUPPORTED)?;
        provider(self.key, id)
    }
}
impl ISelectionProviderImpl for Provider {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        self.array(self.read(|t, n| {
            t.children(n.id)
                .filter(|n| n.states.contains(States::SELECTED))
                .map(|n| n.id)
                .collect()
        })?)
    }
    fn get_CanSelectMultiple(&self) -> Result<BOOL> {
        self.read(|_, n| BOOL(n.states.contains(States::MULTISELECTABLE) as i32))
    }
    fn get_IsSelectionRequired(&self) -> Result<BOOL> {
        self.read(|_, n| BOOL(n.states.contains(States::REQUIRED) as i32))
    }
}
impl IExpandCollapseProviderImpl for Provider {
    fn Expand(&self) -> Result<()> {
        self.action(Actions::EXPAND, ActionArg::None)
    }
    fn Collapse(&self) -> Result<()> {
        self.action(Actions::COLLAPSE, ActionArg::None)
    }
    fn get_ExpandCollapseState(&self) -> Result<ExpandCollapseState> {
        self.read(|_, n| expand(n.states))
    }
}
impl IScrollItemProviderImpl for Provider {
    fn ScrollIntoView(&self) -> Result<()> {
        self.action(Actions::SCROLL_INTO_VIEW, ActionArg::None)
    }
}

/// The model publishes window-local content bounds, so derive the viewport's offset and
/// extent from its children. No second scroll state is kept in the OS adapter.
#[derive(Clone, Copy)]
struct ScrollAxis {
    visible: f64,
    total: f64,
    offset: f64,
}
impl ScrollAxis {
    fn scrollable(self) -> bool {
        self.total > self.visible
    }
    fn percent(self) -> f64 {
        if self.scrollable() {
            100.0 * self.offset / (self.total - self.visible)
        } else {
            -1.0
        }
    }
    fn view(self) -> f64 {
        if self.total > 0.0 {
            (100.0 * self.visible / self.total).min(100.0)
        } else {
            100.0
        }
    }
    fn delta(self, amount: ScrollAmount) -> Result<f32> {
        let fraction = match amount {
            v if v == ScrollAmount_NoAmount => 0.0,
            v if v == ScrollAmount_LargeIncrement => 1.0,
            v if v == ScrollAmount_LargeDecrement => -1.0,
            v if v == ScrollAmount_SmallIncrement => 0.1,
            v if v == ScrollAmount_SmallDecrement => -0.1,
            _ => return Err(E_INVALIDARG),
        };
        if fraction != 0.0 && !self.scrollable() {
            return Err(UNSUPPORTED);
        }
        Ok((fraction * self.visible) as f32)
    }
    fn to_percent(self, percent: f64) -> Result<f32> {
        if percent == -1.0 {
            return Ok(0.0);
        }
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return Err(E_INVALIDARG);
        }
        if !self.scrollable() {
            return Err(UNSUPPORTED);
        }
        Ok(((self.total - self.visible) * percent / 100.0 - self.offset) as f32)
    }
}
impl Provider {
    fn scroll(&self) -> Result<[ScrollAxis; 2]> {
        self.read(|t, n| {
            let r = n.bounds;
            let mut lo = [r.x, r.y];
            let mut hi = [r.x + r.w, r.y + r.h];
            for child in t.children(n.id) {
                let c = child.bounds;
                lo[0] = lo[0].min(c.x);
                lo[1] = lo[1].min(c.y);
                hi[0] = hi[0].max(c.x + c.w);
                hi[1] = hi[1].max(c.y + c.h);
            }
            [
                ScrollAxis {
                    visible: r.w as f64,
                    total: (hi[0] - lo[0]) as f64,
                    offset: (r.x - lo[0]) as f64,
                },
                ScrollAxis {
                    visible: r.h as f64,
                    total: (hi[1] - lo[1]) as f64,
                    offset: (r.y - lo[1]) as f64,
                },
            ]
        })
    }
}
impl IScrollProviderImpl for Provider {
    fn Scroll(&self, h: ScrollAmount, v: ScrollAmount) -> Result<()> {
        let [x, y] = self.scroll()?;
        self.action(Actions::SCROLL, ActionArg::Scroll(x.delta(h)?, y.delta(v)?))
    }
    fn SetScrollPercent(&self, h: f64, v: f64) -> Result<()> {
        let [x, y] = self.scroll()?;
        self.action(
            Actions::SCROLL,
            ActionArg::Scroll(x.to_percent(h)?, y.to_percent(v)?),
        )
    }
    fn get_HorizontalScrollPercent(&self) -> Result<f64> {
        Ok(self.scroll()?[0].percent())
    }
    fn get_VerticalScrollPercent(&self) -> Result<f64> {
        Ok(self.scroll()?[1].percent())
    }
    fn get_HorizontalViewSize(&self) -> Result<f64> {
        Ok(self.scroll()?[0].view())
    }
    fn get_VerticalViewSize(&self) -> Result<f64> {
        Ok(self.scroll()?[1].view())
    }
    fn get_HorizontallyScrollable(&self) -> Result<BOOL> {
        Ok(BOOL(self.scroll()?[0].scrollable() as i32))
    }
    fn get_VerticallyScrollable(&self) -> Result<BOOL> {
        Ok(BOOL(self.scroll()?[1].scrollable() as i32))
    }
}
fn descendants(t: &Tree, id: NodeId) -> Vec<NodeId> {
    let mut todo = vec![id];
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(n) = t.node(id) {
            out.push(id);
            todo.extend(n.children.iter().rev());
        }
    }
    out
}
impl IGridProviderImpl for Provider {
    fn GetItem(&self, row: i32, col: i32) -> Result<IRawElementProviderSimple> {
        let info = self.table()?;
        if row < 0 || col < 0 || row as u32 >= info.rows || col as u32 >= info.cols {
            return Err(E_INVALIDARG);
        }
        let id = self
            .read(|t, n| {
                descendants(t, n.id).into_iter().skip(1).find(|id| {
                    t.node(*id).is_some_and(|c| {
                        !matches!(c.role, Role::Row | Role::Grid | Role::Table)
                            && c.table.is_some_and(|i| {
                                row as u32 >= i.row
                                    && (row as u32) < i.row.saturating_add(i.row_span.max(1))
                                    && col as u32 >= i.col
                                    && (col as u32) < i.col.saturating_add(i.col_span.max(1))
                            })
                    })
                })
            })?
            .ok_or(UNSUPPORTED)?;
        provider(self.key, id)
    }
    fn get_RowCount(&self) -> Result<i32> {
        Ok(self.table()?.rows as i32)
    }
    fn get_ColumnCount(&self) -> Result<i32> {
        Ok(self.table()?.cols as i32)
    }
}
impl IGridItemProviderImpl for Provider {
    fn get_Row(&self) -> Result<i32> {
        Ok(self.table()?.row as i32)
    }
    fn get_Column(&self) -> Result<i32> {
        Ok(self.table()?.col as i32)
    }
    fn get_RowSpan(&self) -> Result<i32> {
        Ok(self.table()?.row_span.max(1) as i32)
    }
    fn get_ColumnSpan(&self) -> Result<i32> {
        Ok(self.table()?.col_span.max(1) as i32)
    }
    fn get_ContainingGrid(&self) -> Result<IRawElementProviderSimple> {
        provider(
            self.key,
            self.ancestor(|n| n.table.is_some() && matches!(n.role, Role::Grid | Role::Table))?,
        )
    }
}
impl Provider {
    fn headers(&self, role: Role, item: bool) -> Result<*mut SAFEARRAY> {
        let grid = if item {
            self.ancestor(|n| n.table.is_some() && matches!(n.role, Role::Grid | Role::Table))?
        } else {
            self.read(|_, n| n.id)?
        };
        let ids = self.read(|t, n| {
            descendants(t, grid)
                .into_iter()
                .filter(|id| {
                    t.node(*id).is_some_and(|header| {
                        header.role == role
                            && (!item
                                || header.table.zip(n.table).is_some_and(|(h, n)| {
                                    if role == Role::RowHeader {
                                        h.row < n.row.saturating_add(n.row_span.max(1))
                                            && n.row < h.row.saturating_add(h.row_span.max(1))
                                    } else {
                                        h.col < n.col.saturating_add(n.col_span.max(1))
                                            && n.col < h.col.saturating_add(h.col_span.max(1))
                                    }
                                }))
                    })
                })
                .collect()
        })?;
        self.array(ids)
    }
}
impl ITableProviderImpl for Provider {
    fn GetRowHeaders(&self) -> Result<*mut SAFEARRAY> {
        self.headers(Role::RowHeader, false)
    }
    fn GetColumnHeaders(&self) -> Result<*mut SAFEARRAY> {
        self.headers(Role::ColumnHeader, false)
    }
    fn get_RowOrColumnMajor(&self) -> Result<RowOrColumnMajor> {
        self.read(|_, _| RowOrColumnMajor_RowMajor)
    }
}
impl ITableItemProviderImpl for Provider {
    fn GetRowHeaderItems(&self) -> Result<*mut SAFEARRAY> {
        self.headers(Role::RowHeader, true)
    }
    fn GetColumnHeaderItems(&self) -> Result<*mut SAFEARRAY> {
        self.headers(Role::ColumnHeader, true)
    }
}

// Ranges own UTF-16 endpoints; only the mirror owns text. The weak identity index lets
// Compare/MoveEndpointByRange inspect ranges from this provider without raw COM casts.
struct TextRange {
    owner: Provider,
    endpoints: Mutex<(u32, u32)>,
    identity: std::sync::atomic::AtomicUsize,
}
struct RangeSlot(Arc<TextRange>);
impl std::ops::Deref for RangeSlot {
    type Target = TextRange;
    fn deref(&self) -> &TextRange {
        &self.0
    }
}
fn ranges() -> &'static Mutex<HashMap<usize, Weak<TextRange>>> {
    static RANGES: OnceLock<Mutex<HashMap<usize, Weak<TextRange>>>> = OnceLock::new();
    RANGES.get_or_init(Default::default)
}
impl Drop for TextRange {
    fn drop(&mut self) {
        lock(ranges()).remove(&self.identity.load(std::sync::atomic::Ordering::Relaxed));
    }
}
fn make_range(owner: Provider, start: u32, end: u32) -> Result<ITextRangeProvider> {
    let range = Arc::new(TextRange {
        owner,
        endpoints: Mutex::new((start.min(end), start.max(end))),
        identity: std::sync::atomic::AtomicUsize::new(0),
    });
    let raw = ObjectBuilder::new()
        .add_ITextRangeProvider(Box::new(RangeSlot(range.clone())))
        .finish()?;
    range
        .identity
        .store(raw as usize, std::sync::atomic::Ordering::Relaxed);
    lock(ranges()).insert(raw as usize, Arc::downgrade(&range));
    unsafe { ITextRangeProvider::from_raw(raw) }
}
fn range_array(ranges: Vec<ITextRangeProvider>) -> Result<*mut SAFEARRAY> {
    let items = ranges
        .iter()
        .map(|r| unsafe { IUnknown::query(r.as_raw()) })
        .collect::<Result<Vec<_>>>()?;
    Ok(SafeArray::from_unknowns(&items)?.into_raw())
}
impl Provider {
    fn byte_range(&self, start: u32, end: u32) -> Result<ITextRangeProvider> {
        let t = self.text()?;
        make_range(
            *self,
            text::byte_to_utf16(&t.content, start),
            text::byte_to_utf16(&t.content, end),
        )
    }
}
impl ITextProvider2Impl for Provider {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let t = self.text()?;
        if !self.read(|_, n| n.actions.contains(Actions::SET_TEXT_SELECTION))? {
            return range_array(Vec::new());
        }
        range_array(vec![make_range(
            *self,
            text::byte_to_utf16(&t.content, t.selection.0),
            text::byte_to_utf16(&t.content, t.selection.1),
        )?])
    }
    fn GetVisibleRanges(&self) -> Result<*mut SAFEARRAY> {
        let t = self.text()?;
        let (bounds, hidden) = self.read(|_, n| {
            (
                n.bounds,
                n.states.intersects(States::HIDDEN | States::OFFSCREEN),
            )
        })?;
        if hidden {
            return range_array(Vec::new());
        }
        if t.lines.is_empty() {
            return range_array(vec![ITextProvider2Impl::get_DocumentRange(self)?]);
        }
        let mut result = Vec::new();
        for (i, line) in t.lines.iter().enumerate() {
            let r = line.rect;
            if r.x < bounds.x + bounds.w
                && r.x + r.w > bounds.x
                && r.y < bounds.y + bounds.h
                && r.y + r.h > bounds.y
            {
                let (s, e) = text::line_range(&t, i);
                result.push(make_range(
                    *self,
                    text::byte_to_utf16(&t.content, s),
                    text::byte_to_utf16(&t.content, e),
                )?);
            }
        }
        range_array(result)
    }
    fn RangeFromChild(
        &self,
        child: Option<&IRawElementProviderSimple>,
    ) -> Result<ITextRangeProvider> {
        self.text()?;
        let child = child.ok_or(E_INVALIDARG)?;
        let own = self.simple()?;
        let same = unsafe {
            IUnknown::query(child.as_raw())?.as_raw() == IUnknown::query(own.as_raw())?.as_raw()
        };
        if same {
            ITextProvider2Impl::get_DocumentRange(self)
        } else {
            Err(E_INVALIDARG)
        }
    }
    fn RangeFromPoint(&self, p: UiaPoint) -> Result<ITextRangeProvider> {
        let t = self.text()?;
        let (x, y) = local_point(self.hwnd, p.x, p.y);
        let b = text::byte_at_point(&t, x, y);
        let u = text::byte_to_utf16(&t.content, b);
        make_range(*self, u, u)
    }
    fn get_DocumentRange(&self) -> Result<ITextRangeProvider> {
        let t = self.text()?;
        make_range(*self, 0, t.content.encode_utf16().count() as u32)
    }
    fn get_SupportedTextSelection(&self) -> Result<SupportedTextSelection> {
        self.read(|_, n| {
            if n.actions.contains(Actions::SET_TEXT_SELECTION) {
                SupportedTextSelection_Single
            } else {
                SupportedTextSelection_None
            }
        })
    }
    fn RangeFromAnnotation(
        &self,
        _: Option<&IRawElementProviderSimple>,
    ) -> Result<ITextRangeProvider> {
        self.text()?;
        Err(UNSUPPORTED)
    }
    fn GetCaretRange(&self, active: *mut BOOL) -> Result<ITextRangeProvider> {
        if active.is_null() {
            return Err(E_POINTER);
        }
        let (t, focused) =
            self.read(|_, n| (n.text.clone(), n.states.contains(States::FOCUSED)))?;
        let t = t.ok_or(UNSUPPORTED)?;
        unsafe {
            *active = BOOL(focused as i32);
        }
        let u = text::byte_to_utf16(&t.content, t.selection.1);
        make_range(*self, u, u)
    }
}
fn endpoint_index(e: TextPatternRangeEndpoint) -> Result<usize> {
    match e {
        v if v == TextPatternRangeEndpoint_Start => Ok(0),
        v if v == TextPatternRangeEndpoint_End => Ok(1),
        _ => Err(E_INVALIDARG),
    }
}
fn unit_boundaries(t: &TextInfo, unit: TextUnit) -> Result<Vec<u32>> {
    let s = &t.content;
    let mut bytes = vec![0];
    match unit {
        v if v == TextUnit_Character => bytes.extend(s.char_indices().map(|(b, _)| b as u32)),
        v if v == TextUnit_Word => {
            let mut previous = None;
            for (b, c) in s.char_indices() {
                let class = if c.is_whitespace() {
                    0
                } else if c.is_alphanumeric() || c == '_' {
                    1
                } else {
                    2
                };
                if class != 0 && previous != Some(class) {
                    bytes.push(b as u32);
                }
                previous = Some(class);
            }
        }
        v if (v == TextUnit_Line) && (!t.lines.is_empty()) => {
            bytes.extend(t.lines.iter().map(|l| l.start))
        }
        v if v == TextUnit_Line || v == TextUnit_Paragraph => {
            bytes.extend(s.match_indices('\n').map(|(b, _)| (b + 1) as u32))
        }
        // Formatting and page boundaries are not in the mirror; promote to Document.
        v if v == TextUnit_Format || v == TextUnit_Page || v == TextUnit_Document => (),
        _ => return Err(E_INVALIDARG),
    }
    bytes.push(s.len() as u32);
    bytes.sort_unstable();
    bytes.dedup();
    Ok(bytes
        .into_iter()
        .map(|b| text::byte_to_utf16(s, b))
        .collect())
}
impl TextRange {
    fn snapshot(&self) -> Result<(TextInfo, (u32, u32))> {
        let t = self.owner.text()?;
        let endpoints = *lock(&self.endpoints);
        // Clamp stale ranges after editing and never expose a split surrogate pair.
        let clamp = |u| text::byte_to_utf16(&t.content, text::utf16_to_byte(&t.content, u));
        let endpoints = (clamp(endpoints.0), clamp(endpoints.1));
        Ok((t, endpoints))
    }
    fn other(&self, other: Option<&ITextRangeProvider>) -> Result<Arc<TextRange>> {
        self.owner.text()?;
        let other = other.ok_or(E_INVALIDARG)?;
        let identity = unsafe { IUnknown::query(other.as_raw())? };
        let range = lock(ranges())
            .get(&(identity.as_raw() as usize))
            .and_then(Weak::upgrade)
            .ok_or(E_INVALIDARG)?;
        if range.owner.key != self.owner.key || range.owner.id != self.owner.id {
            return Err(E_INVALIDARG);
        }
        range.owner.text()?;
        Ok(range)
    }
    fn set_endpoint(&self, endpoint: usize, value: u32, mut current: (u32, u32)) {
        if endpoint == 0 {
            current.0 = value;
            current.1 = current.1.max(value);
        } else {
            current.1 = value;
            current.0 = current.0.min(value);
        }
        *lock(&self.endpoints) = current;
    }
    fn select(&self) -> Result<()> {
        let (t, (s, e)) = self.snapshot()?;
        self.owner.action(
            Actions::SET_TEXT_SELECTION,
            ActionArg::Range(
                text::utf16_to_byte(&t.content, s),
                text::utf16_to_byte(&t.content, e),
            ),
        )
    }
}
impl ITextRangeProviderImpl for RangeSlot {
    fn Clone(&self) -> Result<ITextRangeProvider> {
        let (_, (s, e)) = self.snapshot()?;
        make_range(self.owner, s, e)
    }
    fn Compare(&self, other: Option<&ITextRangeProvider>) -> Result<BOOL> {
        let a = self.snapshot()?.1;
        let other = self.other(other)?;
        Ok(BOOL((a == other.snapshot()?.1) as i32))
    }
    fn CompareEndpoints(
        &self,
        e: TextPatternRangeEndpoint,
        other: Option<&ITextRangeProvider>,
        te: TextPatternRangeEndpoint,
    ) -> Result<i32> {
        let a = self.snapshot()?.1;
        let b = self.other(other)?.snapshot()?.1;
        let a = [a.0, a.1][endpoint_index(e)?];
        let b = [b.0, b.1][endpoint_index(te)?];
        Ok((a as i64 - b as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }
    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> Result<()> {
        let (t, (s, _)) = self.snapshot()?;
        let b = unit_boundaries(&t, unit)?;
        let i = b
            .partition_point(|p| *p <= s)
            .saturating_sub(1)
            .min(b.len().saturating_sub(2));
        *lock(&self.endpoints) = (b[i], b.get(i + 1).copied().unwrap_or(b[i]));
        Ok(())
    }
    fn FindAttribute(
        &self,
        id: UIA_TEXTATTRIBUTE_ID,
        val: VARIANT,
        _: BOOL,
    ) -> Result<Option<ITextRangeProvider>> {
        let own = unsafe { Variant::from_raw(self.GetAttributeValue(id)?) };
        if id == UIA_IsReadOnlyAttributeId
            && unsafe {
                val.Anonymous.Anonymous.vt == VT_BOOL
                    && val.Anonymous.Anonymous.Anonymous.boolVal.0
                        == own.as_raw().Anonymous.Anonymous.Anonymous.boolVal.0
            }
        {
            self.Clone().map(Some)
        } else {
            Ok(None)
        }
    }
    fn FindText(
        &self,
        needle: *mut u16,
        backward: BOOL,
        ignore_case: BOOL,
    ) -> Result<Option<ITextRangeProvider>> {
        if needle.is_null() {
            return Err(E_INVALIDARG);
        }
        let needle = unsafe {
            String::from_utf16_lossy(std::slice::from_raw_parts(
                needle,
                SysStringLen(Some(needle)) as usize,
            ))
        };
        if needle.is_empty() {
            return Err(E_INVALIDARG);
        }
        let (t, (s, e)) = self.snapshot()?;
        let start = text::utf16_to_byte(&t.content, s) as usize;
        let end = text::utf16_to_byte(&t.content, e) as usize;
        let hay = &t.content[start..end];
        // Map case-folded bytes back to original char boundaries (Unicode lowercase can grow).
        let (search, query, mapping) = if ignore_case.as_bool() {
            let mut folded = String::new();
            let mut map = Vec::new();
            for (i, c) in hay.char_indices() {
                for lower in c.to_lowercase() {
                    let old = folded.len();
                    folded.push(lower);
                    map.resize(folded.len(), i);
                    debug_assert!(folded.len() > old);
                }
            }
            map.push(hay.len());
            (folded, needle.to_lowercase(), Some(map))
        } else {
            (hay.to_string(), needle, None)
        };
        let found = if backward.as_bool() {
            search.rfind(&query)
        } else {
            search.find(&query)
        };
        let Some(i) = found else {
            return Ok(None);
        };
        let (a, b) = if let Some(map) = mapping {
            let a = map[i];
            let last = map[i + query.len() - 1];
            let b = last + hay[last..].chars().next().map_or(0, char::len_utf8);
            (a, b)
        } else {
            (i, i + query.len())
        };
        self.owner
            .byte_range((start + a) as u32, (start + b) as u32)
            .map(Some)
    }
    fn GetAttributeValue(&self, id: UIA_TEXTATTRIBUTE_ID) -> Result<VARIANT> {
        let read_only = self.owner.read(|_, n| {
            n.states.contains(States::READ_ONLY) || !n.states.contains(States::EDITABLE)
        })?;
        if id == UIA_IsReadOnlyAttributeId {
            return Ok(Variant::from_bool(read_only).into_raw());
        }
        // UIA's reserved value distinguishes unsupported attributes from an empty value.
        let unknown = unsafe { UiaGetReservedNotSupportedValue()? };
        Ok(Variant::from_unknown(&unknown).into_raw())
    }
    fn GetBoundingRectangles(&self) -> Result<*mut SAFEARRAY> {
        let (t, (s, e)) = self.snapshot()?;
        let rects = text::range_rects(
            &t,
            text::utf16_to_byte(&t.content, s),
            text::utf16_to_byte(&t.content, e),
        );
        let values: Vec<f64> = rects
            .into_iter()
            .flat_map(|r| {
                let r = screen_rect(self.owner.hwnd, r);
                [r.left, r.top, r.width, r.height]
            })
            .collect();
        Ok(doubles(&values)?.into_raw())
    }
    fn GetEnclosingElement(&self) -> Result<IRawElementProviderSimple> {
        self.owner.simple()
    }
    fn GetText(&self, max: i32) -> Result<*mut u16> {
        if max < -1 {
            return Err(E_INVALIDARG);
        }
        let (t, (s, e)) = self.snapshot()?;
        let e = if max < 0 {
            e
        } else {
            e.min(s.saturating_add(max as u32))
        };
        let a = text::utf16_to_byte(&t.content, s) as usize;
        let b = text::utf16_to_byte(&t.content, e) as usize;
        bstr(&t.content[a..b])
    }
    fn Move(&self, unit: TextUnit, count: i32) -> Result<i32> {
        let (t, (s, e)) = self.snapshot()?;
        if count == 0 {
            return Ok(0);
        }
        let b = unit_boundaries(&t, unit)?;
        let max = b.len().saturating_sub(if s == e { 1 } else { 2 });
        let old = b.partition_point(|p| *p <= s).saturating_sub(1).min(max);
        let new = (old as i64 + count as i64).clamp(0, max as i64) as usize;
        *lock(&self.endpoints) = (
            b[new],
            if s == e {
                b[new]
            } else {
                b.get(new + 1).copied().unwrap_or(b[new])
            },
        );
        Ok(new as i32 - old as i32)
    }
    fn MoveEndpointByUnit(
        &self,
        endpoint: TextPatternRangeEndpoint,
        unit: TextUnit,
        count: i32,
    ) -> Result<i32> {
        let index = endpoint_index(endpoint)?;
        let (t, current) = self.snapshot()?;
        if count == 0 {
            return Ok(0);
        }
        let b = unit_boundaries(&t, unit)?;
        let value = [current.0, current.1][index];
        let old = if count > 0 {
            b.partition_point(|p| *p <= value).saturating_sub(1)
        } else {
            b.partition_point(|p| *p < value)
        };
        let new = (old as i64 + count as i64).clamp(0, b.len() as i64 - 1) as usize;
        self.set_endpoint(index, b[new], current);
        Ok(new as i32 - old as i32)
    }
    fn MoveEndpointByRange(
        &self,
        endpoint: TextPatternRangeEndpoint,
        other: Option<&ITextRangeProvider>,
        target: TextPatternRangeEndpoint,
    ) -> Result<()> {
        let current = self.snapshot()?.1;
        let other = self.other(other)?.snapshot()?.1;
        self.set_endpoint(
            endpoint_index(endpoint)?,
            [other.0, other.1][endpoint_index(target)?],
            current,
        );
        Ok(())
    }
    fn Select(&self) -> Result<()> {
        self.select()
    }
    fn AddToSelection(&self) -> Result<()> {
        self.owner.text()?;
        Err(UNSUPPORTED)
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        self.owner.text()?;
        Err(UNSUPPORTED)
    }
    fn ScrollIntoView(&self, _: BOOL) -> Result<()> {
        self.owner
            .action(Actions::SCROLL_INTO_VIEW, ActionArg::None)
    }
    fn GetChildren(&self) -> Result<*mut SAFEARRAY> {
        self.owner.text()?;
        Ok(SafeArray::from_unknowns(&[])?.into_raw())
    }
}

#[derive(Clone)]
enum PropertyValue {
    Text(String),
    Number(f64),
    Int(i32),
    Bool(bool),
    Bounds(Rect),
    Empty,
}
impl PropertyValue {
    fn variant(&self, hwnd: usize) -> Result<Variant> {
        Ok(match self {
            Self::Text(s) => Variant::from_bstr(s)?,
            Self::Number(n) => Variant::from_f64(*n),
            Self::Int(n) => Variant::from_i32(*n),
            Self::Bool(b) => Variant::from_bool(*b),
            Self::Bounds(r) => rect_variant(screen_rect(hwnd, *r))?,
            Self::Empty => Variant::empty(),
        })
    }
}
struct Change {
    node: NodeId,
    property: UIA_PROPERTY_ID,
    old: PropertyValue,
    new: PropertyValue,
}
#[derive(Default)]
pub(crate) struct Update {
    changes: Vec<Change>,
    removed: Vec<NodeId>,
    notifications: Vec<Notification>,
}
/// Called before apply_update, under the HAL lock. Only reads/copies changed semantic
/// fields; no COM or Win32 call, and no full-tree clone on an incremental frame.
pub(crate) fn prepare(key: u64, update: &TreeUpdate) -> Update {
    let start = super::log_enabled().then(std::time::Instant::now);
    let mut result = Update {
        removed: update.removed.clone(),
        ..Default::default()
    };
    with_tree(key, |t| {
        if update.full {
            let present: HashSet<_> = update.nodes.iter().map(|n| n.id).collect();
            result
                .removed
                .extend(t.nodes.keys().filter(|id| !present.contains(id)).copied());
        }
        for new in &update.nodes {
            let Some(old) = t.node(new.id) else { continue };
            let mut change = |property, before, after| {
                result.changes.push(Change {
                    node: new.id,
                    property,
                    old: before,
                    new: after,
                })
            };
            if old.name != new.name {
                change(
                    UIA_NamePropertyId,
                    PropertyValue::Text(old.name.clone()),
                    PropertyValue::Text(new.name.clone()),
                );
            }
            if old.value != new.value {
                change(
                    UIA_ValueValuePropertyId,
                    PropertyValue::Text(old.value.clone()),
                    PropertyValue::Text(new.value.clone()),
                );
            }
            if let Some((a, b)) = old.range.zip(new.range).filter(|(a, b)| a.now != b.now) {
                change(
                    UIA_RangeValueValuePropertyId,
                    PropertyValue::Number(a.now),
                    PropertyValue::Number(b.now),
                );
            }
            if old.toggle != new.toggle {
                change(
                    UIA_ToggleToggleStatePropertyId,
                    PropertyValue::Int(toggle(old.toggle).0),
                    PropertyValue::Int(toggle(new.toggle).0),
                );
            }
        }
    });
    for notification in &update.notifications {
        if let Notification::StatesChanged { node, old, new } = notification {
            let mut change = |property, before, after| {
                result.changes.push(Change {
                    node: *node,
                    property,
                    old: before,
                    new: after,
                })
            };
            if expand(*old) != expand(*new) {
                change(
                    UIA_ExpandCollapseExpandCollapseStatePropertyId,
                    PropertyValue::Int(expand(*old).0),
                    PropertyValue::Int(expand(*new).0),
                );
            }
            for (state, property, invert) in [
                (
                    States::SELECTED,
                    UIA_SelectionItemIsSelectedPropertyId,
                    false,
                ),
                (States::DISABLED, UIA_IsEnabledPropertyId, true),
                (States::FOCUSED, UIA_HasKeyboardFocusPropertyId, false),
            ] {
                if old.contains(state) != new.contains(state) {
                    change(
                        property,
                        PropertyValue::Bool(old.contains(state) ^ invert),
                        PropertyValue::Bool(new.contains(state) ^ invert),
                    );
                }
            }
        }
    }
    if let Some(start) = start {
        let nanos = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        use std::sync::atomic::Ordering::Relaxed;
        PREPARE_UPDATES.fetch_add(1, Relaxed);
        PREPARE_NODES.fetch_add(update.nodes.len() as u64, Relaxed);
        PREPARE_NS.fetch_add(nanos, Relaxed);
        PREPARE_MAX_NS.fetch_max(nanos, Relaxed);
    }
    result
}
static PREPARE_UPDATES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PREPARE_NODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PREPARE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PREPARE_MAX_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) fn queue(
    shell: &super::Shared,
    key: u64,
    mut update: Update,
    notifications: &[Notification],
) {
    update.notifications = notifications.to_vec();
    if !update.changes.is_empty() || !update.removed.is_empty() || !update.notifications.is_empty()
    {
        shell.post(super::Op::A11yNotify(key, update));
    }
}
pub(super) fn notify(key: u64, update: Update) {
    let retired = {
        let mut windows = lock(windows());
        let Some(w) = windows.get_mut(&key) else {
            return;
        };
        update
            .removed
            .iter()
            .filter_map(|id| w.providers.remove(id))
            .collect::<Vec<_>>()
    };
    disconnect(retired);
    if !unsafe { UiaClientsAreListening().as_bool() } {
        return;
    }
    for c in update.changes {
        if let (Ok(p), Ok(old), Ok(new)) =
            (provider(key, c.node), c.old.variant(0), c.new.variant(0))
        {
            unsafe {
                let _ = UiaRaiseAutomationPropertyChangedEvent(
                    &p,
                    c.property,
                    *old.as_raw(),
                    *new.as_raw(),
                );
            }
        }
    }
    for notification in update.notifications {
        let (node, event) = match notification {
            Notification::FocusChanged(id) => (id, UIA_AutomationFocusChangedEventId),
            Notification::TextChanged { node, .. } => (node, UIA_Text_TextChangedEventId),
            Notification::CaretMoved(id) => (id, UIA_Text_TextSelectionChangedEventId),
            Notification::LiveRegionChanged(id) => (id, UIA_LiveRegionChangedEventId),
            Notification::ChildrenChanged(id) => {
                if let Ok(p) = provider(key, id) {
                    unsafe {
                        let _ = UiaRaiseStructureChangedEvent(
                            &p,
                            StructureChangeType_ChildrenInvalidated,
                            std::ptr::null_mut(),
                            0,
                        );
                    }
                }
                continue;
            }
            Notification::Announce { text, live } => {
                if let (Ok(p), Ok(message), Ok(activity)) = (
                    provider(key, NodeId::NONE),
                    BSTR::from_str(&text),
                    BSTR::from_str(""),
                ) {
                    let processing = if live == Live::Assertive {
                        NotificationProcessing_ImportantAll
                    } else {
                        NotificationProcessing_All
                    };
                    unsafe {
                        let _ = UiaRaiseNotificationEvent(
                            &p,
                            NotificationKind_Other,
                            processing,
                            Some(message.0),
                            activity.0,
                        );
                    }
                }
                continue;
            }
            Notification::StatesChanged { node, old, new }
                if old.contains(States::SELECTED) != new.contains(States::SELECTED) =>
            {
                (
                    node,
                    if new.contains(States::SELECTED) {
                        UIA_SelectionItem_ElementSelectedEventId
                    } else {
                        UIA_SelectionItem_ElementRemovedFromSelectionEventId
                    },
                )
            }
            _ => continue,
        };
        if let Ok(p) = provider(key, node) {
            unsafe {
                let _ = UiaRaiseAutomationEvent(&p, event);
            }
        }
    }
}

struct ActionWatch {
    key: u64,
    node: NodeId,
    req: Option<u64>,
}
fn action_watch() -> &'static Mutex<Option<ActionWatch>> {
    static WATCH: OnceLock<Mutex<Option<ActionWatch>>> = OnceLock::new();
    WATCH.get_or_init(Default::default)
}
fn action_signal() -> &'static std::sync::Condvar {
    static SIGNAL: std::sync::Condvar = std::sync::Condvar::new();
    &SIGNAL
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
fn client_bounds(element: &IUIAutomationElement) -> Result<[f64; 4]> {
    unsafe {
        let value =
            Variant::from_raw(element.GetCurrentPropertyValue(UIA_BoundingRectanglePropertyId)?);
        if value.as_raw().Anonymous.Anonymous.vt != VARENUM(VT_ARRAY.0 | VT_R8.0) {
            return Err(E_INVALIDARG);
        }
        let array = value.as_raw().Anonymous.Anonymous.Anonymous.parray;
        if array.is_null() || (*array).cDims != 1 || (*array).rgsabound[0].cElements != 4 {
            return Err(E_INVALIDARG);
        }
        let data = SafeArrayAccessData(array)?;
        let mut rect = [0.0; 4];
        std::ptr::copy_nonoverlapping(data.cast::<f64>(), rect.as_mut_ptr(), 4);
        SafeArrayUnaccessData(array)?;
        Ok(rect)
    }
}
fn selfcheck(key: u64, act: bool) -> String {
    static CHECK: Mutex<()> = Mutex::new(());
    let _check = lock(&CHECK); // Remote thread only; serialize action observation.
    let start = std::time::Instant::now();
    let result = (|| -> Result<String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let _apartment = Apartment;
            let client = IUIAutomation::from_raw(CoCreateInstance(
                &CUIAutomation,
                None,
                CLSCTX_INPROC_SERVER,
                &IUIAutomation::IID,
            )?)?;
            let hwnd = lock(windows()).get(&key).map(|w| w.hwnd).ok_or(GONE)?;
            let root = client.ElementFromHandle(HWND(hwnd as _))?;
            let walker = client.get_RawViewWalker()?;
            let tree = with_tree(key, Clone::clone).ok_or(GONE)?;
            let mut pending = vec![(root, Some(tree.root))];
            let mut seen = HashSet::new();
            let mut nodes = 0;
            let mut mismatches = Vec::new();
            let mut invoke = None;
            while let Some((element, expected)) = pending.pop() {
                nodes += 1;
                let Some(id) = expected else {
                    mismatches.push("unexpected UIA child".to_string());
                    continue;
                };
                if !seen.insert(id) {
                    mismatches.push(format!("#{} duplicate child", id.short()));
                    continue;
                }
                let Some(n) = tree.node(id) else {
                    mismatches.push(format!("#{} absent in mirror", id.short()));
                    continue;
                };
                let mut compare = |ok: bool, property: &str| {
                    if !ok {
                        mismatches.push(format!("#{} {property}", id.short()));
                    }
                };
                compare(
                    element.get_CurrentControlType()?.0 == n.role.uia_control_type(),
                    "control_type",
                );
                let name = BSTR(element.get_CurrentName()?);
                let value = if name.0.is_null() {
                    String::new()
                } else {
                    String::from_utf16_lossy(std::slice::from_raw_parts(
                        name.0,
                        SysStringLen(Some(name.0)) as usize,
                    ))
                };
                compare(value == n.name, "name");
                compare(
                    element.get_CurrentIsEnabled()?.as_bool()
                        == !n.states.contains(States::DISABLED),
                    "enabled",
                );
                compare(
                    element.get_CurrentIsKeyboardFocusable()?.as_bool()
                        == n.states.contains(States::FOCUSABLE),
                    "focusable",
                );
                // Compare origin and size separately, in the physical pixels exposed by UIA.
                let actual = client_bounds(&element)?;
                let wanted = screen_rect(hwnd, n.bounds);
                let wanted = [wanted.left, wanted.top, wanted.width, wanted.height];
                compare(
                    actual
                        .into_iter()
                        .zip(wanted)
                        .all(|(a, b)| (a - b).abs() < 0.0001),
                    &format!("bounds UIA={actual:?} mirror={wanted:?}"),
                );
                let supported = patterns(&tree, n);
                for (pattern, bit, name) in PATTERNS {
                    compare(
                        element.GetCurrentPattern(*pattern).is_ok() == (supported & bit != 0),
                        &format!("pattern {name}"),
                    );
                }
                if act
                    && invoke.is_none()
                    && n.actions.contains(Actions::PRESS)
                    && !n.states.contains(States::DISABLED)
                {
                    let pattern = element.GetCurrentPattern(UIA_InvokePatternId)?;
                    invoke = Some((id, IUIAutomationInvokePattern::query(pattern.as_raw())?));
                }
                let mut children = Vec::new();
                let mut child = walker.GetFirstChildElement(&element)?;
                while let Some(c) = child {
                    child = walker.GetNextSiblingElement(&c)?;
                    children.push(c);
                }
                if children.len() != n.children.len() {
                    mismatches.push(format!(
                        "#{} children UIA={} mirror={}",
                        id.short(),
                        children.len(),
                        n.children.len()
                    ));
                }
                for (index, c) in children.into_iter().enumerate().rev() {
                    pending.push((c, n.children.get(index).copied()));
                }
            }
            let mut reachable = Vec::new();
            tree.walk(|n, _| reachable.push(n.id));
            for id in reachable.iter().filter(|id| !seen.contains(id)) {
                mismatches.push(format!("#{} missing", id.short()));
            }
            let mut out = mismatches.join("\n");
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!(
                "uia nodes={nodes} mirror={} mismatches={} elapsed_ms={}",
                reachable.len(),
                mismatches.len(),
                start.elapsed().as_millis()
            ));
            if act {
                if let Some((id, pattern)) = invoke {
                    *lock(action_watch()) = Some(ActionWatch {
                        key,
                        node: id,
                        req: None,
                    });
                    let invoked = pattern.Invoke();
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    let mut watch = lock(action_watch());
                    while invoked.is_ok() && watch.as_ref().is_some_and(|w| w.req.is_none()) {
                        let left = deadline.saturating_duration_since(std::time::Instant::now());
                        if left.is_zero() {
                            break;
                        }
                        watch = action_signal()
                            .wait_timeout(watch, left)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                    let req = watch.take().and_then(|w| w.req);
                    drop(watch);
                    invoked?;
                    let result = req.and_then(|req| {
                        let result = wait_done(req, Duration::from_secs(5));
                        if result.is_none() {
                            crate::a11y::forget_action(req);
                        }
                        result
                    });
                    out.push_str(&format!(
                        "\ninvoke #{} {}",
                        id.short(),
                        match result {
                            Some(r)
                                if r == ActionResult::Handled as u8
                                    || r == ActionResult::Fallback as u8 =>
                                "ok",
                            Some(_) => "failed: app rejected action",
                            None => "failed: no app completion",
                        }
                    ));
                } else {
                    out.push_str("\ninvoke failed: no enabled Invoke node");
                }
            }
            if super::log_enabled() {
                use std::sync::atomic::Ordering::Relaxed;
                out.push_str(&format!(
                    "\nprepare updates={} changed_nodes={} total_us={:.1} max_us={:.1}",
                    PREPARE_UPDATES.load(Relaxed),
                    PREPARE_NODES.load(Relaxed),
                    PREPARE_NS.load(Relaxed) as f64 / 1000.0,
                    PREPARE_MAX_NS.load(Relaxed) as f64 / 1000.0
                ));
            }
            Ok(out)
        }
    })();
    result.unwrap_or_else(|e| format!("uia error {e}"))
}
