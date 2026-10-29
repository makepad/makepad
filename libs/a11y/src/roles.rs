//! The role table: one row per role with its name in every OS accessibility API. The enum and every
//! adapter's mapping are generated from this one table.
//!
//! Columns: `Variant = tag, "short name", uia control type id, AX role, AX subrole, AT-SPI role number,
//! Android class name, ARIA role`. An empty string means "none" for that API (the adapter falls back to
//! the API's generic group role).

macro_rules! roles {
    ($( $v:ident = $tag:literal, $short:literal, $uia:literal, $ax:literal, $axsub:literal, $atspi:literal, $android:literal, $aria:literal; )*) => {
        /// What a node is. Closed set covering what the OS APIs have in common.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u16)]
        pub enum Role { $( $v = $tag, )* }

        impl Role {
            pub const ALL: &'static [Role] = &[$( Role::$v, )*];
            pub fn from_u16(tag: u16) -> Option<Role> {
                match tag { $( $tag => Some(Role::$v), )* _ => None }
            }
            /// The name used by `ui` output and queries (`role=button`).
            pub fn short(self) -> &'static str {
                match self { $( Role::$v => $short, )* }
            }
            pub fn from_short(name: &str) -> Option<Role> {
                match name { $( $short => Some(Role::$v), )* _ => None }
            }
            /// UI Automation control type id (UIA_*ControlTypeId).
            pub fn uia_control_type(self) -> i32 {
                match self { $( Role::$v => $uia, )* }
            }
            /// NSAccessibility role and subrole ("" = none).
            pub fn ax_role(self) -> (&'static str, &'static str) {
                match self { $( Role::$v => ($ax, $axsub), )* }
            }
            /// AT-SPI2 role number (AtspiRole).
            pub fn atspi_role(self) -> u32 {
                match self { $( Role::$v => $atspi, )* }
            }
            /// Android widget class name reported by AccessibilityNodeInfo.getClassName.
            pub fn android_class(self) -> &'static str {
                match self { $( Role::$v => $android, )* }
            }
            /// ARIA role for the web mirror ("" = generic element, no role attribute).
            pub fn aria(self) -> &'static str {
                match self { $( Role::$v => $aria, )* }
            }
        }
    };
}

// uia: 50000 Button, 50002 CheckBox, 50003 ComboBox, 50004 Edit, 50005 Hyperlink, 50006 Image, 50007 ListItem,
// 50008 List, 50009 Menu, 50010 MenuBar, 50011 MenuItem, 50012 ProgressBar, 50013 RadioButton, 50014 ScrollBar,
// 50015 Slider, 50016 Spinner, 50017 StatusBar, 50018 Tab, 50019 TabItem, 50020 Text, 50021 ToolBar,
// 50022 ToolTip, 50023 Tree, 50024 TreeItem, 50025 Custom, 50026 Group, 50028 DataGrid, 50029 DataItem,
// 50030 Document, 50032 Window, 50033 Pane, 50034 Header, 50035 HeaderItem, 50036 Table, 50037 TitleBar,
// 50038 Separator.
// AT-SPI numbers follow AtspiRole (checked against atspi-constants.h of at-spi2-core 2.60.6); an app
// window is a FRAME (23) there.
roles! {
    Generic        = 0,  "generic",        50026, "AXGroup", "", 39, "android.view.View", "";
    Window         = 1,  "window",         50032, "AXWindow", "AXStandardWindow", 23, "android.view.ViewGroup", "application";
    Application    = 2,  "application",    50033, "AXGroup", "", 75, "android.view.ViewGroup", "application";
    Group          = 3,  "group",          50026, "AXGroup", "", 99, "android.view.ViewGroup", "group";
    Pane           = 4,  "pane",           50033, "AXGroup", "", 39, "android.view.ViewGroup", "region";
    Toolbar        = 5,  "toolbar",        50021, "AXToolbar", "", 63, "android.view.ViewGroup", "toolbar";
    Button         = 6,  "button",         50000, "AXButton", "", 43, "android.widget.Button", "button";
    ToggleButton   = 7,  "toggle_button",  50000, "AXCheckBox", "AXToggle", 62, "android.widget.ToggleButton", "button";
    CheckBox       = 8,  "check_box",      50002, "AXCheckBox", "", 7, "android.widget.CheckBox", "checkbox";
    Radio          = 9,  "radio",          50013, "AXRadioButton", "", 44, "android.widget.RadioButton", "radio";
    RadioGroup     = 10, "radio_group",    50026, "AXRadioGroup", "", 99, "android.widget.RadioGroup", "radiogroup";
    Switch         = 11, "switch",         50002, "AXCheckBox", "AXSwitch", 130, "android.widget.Switch", "switch";
    Link           = 12, "link",           50005, "AXLink", "", 88, "android.widget.TextView", "link";
    Menu           = 13, "menu",           50009, "AXMenu", "", 33, "android.view.ViewGroup", "menu";
    MenuBar        = 14, "menu_bar",       50010, "AXMenuBar", "", 34, "android.view.ViewGroup", "menubar";
    MenuItem       = 15, "menu_item",      50011, "AXMenuItem", "", 35, "android.view.View", "menuitem";
    MenuItemCheck  = 16, "menu_item_check",50011, "AXMenuItem", "", 8, "android.widget.CheckBox", "menuitemcheckbox";
    MenuItemRadio  = 17, "menu_item_radio",50011, "AXMenuItem", "", 45, "android.widget.RadioButton", "menuitemradio";
    TabList        = 18, "tab_list",       50018, "AXTabGroup", "", 38, "android.widget.TabWidget", "tablist";
    Tab            = 19, "tab",            50019, "AXRadioButton", "AXTabButton", 37, "android.view.View", "tab";
    TabPanel       = 20, "tab_panel",      50033, "AXGroup", "", 39, "android.view.ViewGroup", "tabpanel";
    List           = 21, "list",           50008, "AXList", "", 31, "android.widget.ListView", "list";
    ListItem       = 22, "list_item",      50007, "AXGroup", "", 32, "android.view.View", "listitem";
    ListBox        = 23, "list_box",       50008, "AXList", "", 98, "android.widget.ListView", "listbox";
    Option         = 24, "option",         50007, "AXStaticText", "", 32, "android.view.View", "option";
    Tree           = 25, "tree",           50023, "AXOutline", "", 65, "android.widget.ListView", "tree";
    TreeItem       = 26, "tree_item",      50024, "AXRow", "AXOutlineRow", 91, "android.view.View", "treeitem";
    Grid           = 27, "grid",           50028, "AXTable", "", 55, "android.widget.GridView", "grid";
    Table          = 28, "table",          50036, "AXTable", "", 55, "android.widget.GridView", "table";
    Row            = 29, "row",            50029, "AXRow", "", 90, "android.view.ViewGroup", "row";
    Cell           = 30, "cell",           50029, "AXCell", "", 56, "android.view.View", "gridcell";
    ColumnHeader   = 31, "column_header",  50035, "AXCell", "", 57, "android.view.View", "columnheader";
    RowHeader      = 32, "row_header",     50035, "AXCell", "", 58, "android.view.View", "rowheader";
    ComboBox       = 33, "combo_box",      50003, "AXPopUpButton", "", 11, "android.widget.Spinner", "combobox";
    Slider         = 34, "slider",         50015, "AXSlider", "", 51, "android.widget.SeekBar", "slider";
    SpinButton     = 35, "spin_button",    50016, "AXIncrementor", "", 52, "android.widget.EditText", "spinbutton";
    ScrollBar      = 36, "scroll_bar",     50014, "AXScrollBar", "", 48, "android.view.View", "scrollbar";
    ScrollArea     = 37, "scroll_area",    50033, "AXScrollArea", "", 49, "android.widget.ScrollView", "";
    Progress       = 38, "progress",       50012, "AXProgressIndicator", "", 42, "android.widget.ProgressBar", "progressbar";
    Meter          = 39, "meter",          50012, "AXLevelIndicator", "", 103, "android.widget.ProgressBar", "meter";
    TextField      = 40, "text_field",     50004, "AXTextField", "", 79, "android.widget.EditText", "textbox";
    TextArea       = 41, "text_area",      50004, "AXTextArea", "", 61, "android.widget.EditText", "textbox";
    SearchField    = 42, "search_field",   50004, "AXTextField", "AXSearchField", 79, "android.widget.EditText", "searchbox";
    PasswordField  = 43, "password_field", 50004, "AXTextField", "AXSecureTextField", 40, "android.widget.EditText", "textbox";
    StaticText     = 44, "static_text",    50020, "AXStaticText", "", 29, "android.widget.TextView", "";
    Heading        = 45, "heading",        50020, "AXHeading", "", 83, "android.widget.TextView", "heading";
    Paragraph      = 46, "paragraph",      50020, "AXGroup", "", 73, "android.widget.TextView", "paragraph";
    Image          = 47, "image",          50006, "AXImage", "", 27, "android.widget.ImageView", "img";
    Figure         = 48, "figure",         50026, "AXGroup", "", 99, "android.view.ViewGroup", "figure";
    Dialog         = 49, "dialog",         50032, "AXGroup", "AXApplicationDialog", 16, "android.app.Dialog", "dialog";
    AlertDialog    = 50, "alert_dialog",   50032, "AXGroup", "AXApplicationAlertDialog", 2, "android.app.Dialog", "alertdialog";
    Alert          = 51, "alert",          50026, "AXGroup", "AXApplicationAlert", 2, "android.view.View", "alert";
    Status         = 52, "status",         50017, "AXGroup", "AXApplicationStatus", 54, "android.view.View", "status";
    Tooltip        = 53, "tooltip",        50022, "AXGroup", "AXUserInterfaceTooltip", 64, "android.view.View", "tooltip";
    Separator      = 54, "separator",      50038, "AXSplitter", "", 50, "android.view.View", "separator";
    Splitter       = 55, "splitter",       50038, "AXSplitter", "", 53, "android.view.View", "separator";
    Canvas         = 56, "canvas",         50025, "AXGroup", "", 6, "android.view.View", "img";
    Document       = 57, "document",       50030, "AXGroup", "AXDocument", 82, "android.view.ViewGroup", "document";
    Log            = 58, "log",            50026, "AXGroup", "AXApplicationLog", 111, "android.view.ViewGroup", "log";
    Timer          = 59, "timer",          50026, "AXGroup", "AXApplicationTimer", 115, "android.view.View", "timer";
    Marquee        = 60, "marquee",        50026, "AXGroup", "AXApplicationMarquee", 112, "android.view.View", "marquee";
    Note           = 61, "note",           50026, "AXGroup", "", 97, "android.view.View", "note";
    TitleBar       = 62, "title_bar",      50037, "AXGroup", "", 104, "android.view.ViewGroup", "";
    Header         = 63, "header",         50034, "AXGroup", "", 71, "android.view.ViewGroup", "banner";
    Footer         = 64, "footer",         50026, "AXGroup", "", 72, "android.view.ViewGroup", "contentinfo";
    Navigation     = 65, "navigation",     50026, "AXGroup", "AXLandmarkNavigation", 110, "android.view.ViewGroup", "navigation";
    Main           = 66, "main",           50026, "AXGroup", "AXLandmarkMain", 110, "android.view.ViewGroup", "main";
    Form           = 67, "form",           50026, "AXGroup", "", 87, "android.view.ViewGroup", "form";
    Terminal       = 68, "terminal",       50030, "AXTextArea", "", 60, "android.widget.EditText", "log";
    Video          = 69, "video",          50025, "AXGroup", "", 107, "android.view.View", "";
    Audio          = 70, "audio",          50025, "AXGroup", "", 106, "android.view.View", "";
    ColorWell      = 71, "color_well",     50000, "AXColorWell", "", 9, "android.widget.Button", "button";
    DisclosureButton=72, "disclosure",     50000, "AXDisclosureTriangle", "", 62, "android.widget.Button", "button";
    KeyboardKey    = 73, "keyboard_key",   50000, "AXButton", "", 43, "android.widget.Button", "button";
}

impl Role {
    /// Roles whose value is user-editable text.
    pub fn is_text_input(self) -> bool {
        matches!(self, Role::TextField | Role::TextArea | Role::SearchField | Role::PasswordField | Role::Terminal | Role::SpinButton)
    }
    /// Roles that are inherently interactive (an unnamed one is a lint).
    pub fn is_interactive(self) -> bool {
        matches!(
            self,
            Role::Button | Role::ToggleButton | Role::CheckBox | Role::Radio | Role::Switch | Role::Link | Role::MenuItem
                | Role::MenuItemCheck | Role::MenuItemRadio | Role::Tab | Role::ComboBox | Role::Slider | Role::SpinButton
                | Role::TextField | Role::TextArea | Role::SearchField | Role::PasswordField | Role::Option | Role::TreeItem
                | Role::ColorWell | Role::DisclosureButton | Role::KeyboardKey
        )
    }
}
