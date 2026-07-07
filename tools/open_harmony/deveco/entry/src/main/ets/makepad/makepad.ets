import display from '@ohos.display';
import deviceInfo from '@ohos.deviceInfo';
import inputMethod from '@ohos.inputMethod';
import { window } from '@kit.ArkUI';
import common from '@ohos.app.ability.common';
import resourceManager from '@ohos.resourceManager';
import font from '@ohos.font';

declare type Any = ESObject;

export interface MakepadXComponentInterface{
  onCreate(ark_ts: object):void;
  handleInsertTextEvent(text:String):void;
  handleDeleteLeftEvent(length: number):void;
  handleKeyboardStatus(isOpen:boolean, keyboardHeight:number):void;
}


function getDensity(): number {
  try {
    let displayClass = display.getDefaultDisplaySync();
    console.info('Test densityDPI:' + JSON.stringify(displayClass.densityDPI));
    return displayClass.densityDPI / 160;
  } catch (exception) {
    console.error('Failed to obtain the default display object. Code: ' + JSON.stringify(exception));
    return 3;
  }
}

function getDeviceType(): string {
  let device_type: string = deviceInfo.deviceType;
  if (device_type == "") {
    console.error("deviceInfo.deviceType is empty string!")
  } else {
    console.info("Device type is " + device_type)
  }
  return device_type;
}

export class ArkGlue{
  private static instance : ArkGlue;
  static initInstance(context: common.Context) : ArkGlue {
    if(ArkGlue.instance == null) {
      ArkGlue.instance = new ArkGlue(context);
    }
      return ArkGlue.instance;
  }

  static getInstance() : ArkGlue {
    return ArkGlue.instance;
  }

  context: common.Context;

  deviceType = getDeviceType();
  osFullName = deviceInfo.osFullName;
  displayDensity = getDensity();
  resMgr : resourceManager.ResourceManager;
  filesDir : string;
  cacheDir : string;
  tempDir : string;

  keyboardHeight : number = 1020;
  xComponentContext : MakepadXComponentInterface | undefined = undefined;

  private inputMethodController : inputMethod.InputMethodController;
  private textConfig: inputMethod.TextConfig;
  private imcFlag = false;

  constructor(context: common.Context) {
    this.context = context;
    this.resMgr = this.context.resourceManager;
    this.filesDir = this.context.filesDir;
    this.cacheDir = this.context.cacheDir;
    this.tempDir = this.context.tempDir;
    this.inputMethodController = inputMethod.getController();
    this.textConfig = {
      inputAttribute: {
        textInputType: 0,
        enterKeyType: 1
      }
    };
  }

  showKeyBoard(): void {
    this.showTextInput();
  }

  hideKeyBoard(): void {
    this.hideTextInput();
  }

  // Resolve system fonts for makepad's runtime font loader. `queryJson` is a JSON
  // object { role, weight, italic, lang, sample } sent from Rust. We enumerate the
  // system fonts via @ohos.font (synchronous, so it can be called from the native
  // uv_loop callback), rank candidates by the requested role/weight/style, and return
  // their file paths newline-separated (best first). Rust reads the bytes and — for the
  // per-glyph `sample` case — verifies cmap coverage itself, since FontInfo carries no
  // charset. Returns "" on any error (older devices without @ohos.font just get no text
  // rather than a crash).
  getSystemFontPaths(queryJson: string): string {
    try {
      let query: Any = JSON.parse(queryJson);
      let role: string = (query.role ?? "ui") as string;
      let wantWeight: number = (query.weight ?? 400) as number;
      let wantItalic: boolean = (query.italic ?? false) as boolean;
      let lang: string = ((query.lang ?? "") as string).toLowerCase();
      let hasSample: boolean = ((query.sample ?? "") as string).length > 0;

      let hints: string[] = ArkGlue.roleHints(role, lang);

      let names: string[] = font.getSystemFontList();
      interface Cand { path: string; rank: number; weight: number; italic: boolean; }
      let cands: Cand[] = [];
      let seen: Set<string> = new Set();
      for (let name of names) {
        let info: font.FontInfo;
        try {
          info = font.getFontByName(name);
        } catch (e) {
          continue;
        }
        if (info == null || info.path == null || info.path.length == 0) {
          continue;
        }
        if (seen.has(info.path)) {
          continue;
        }
        // Normalize family + fullName for substring matching against the role hints.
        let hay: string = ArkGlue.normalizeFontId(
          ((info.family ?? "") as string) + " " + ((info.fullName ?? "") as string) + " " + name);
        let rank: number = hints.length + 1;
        for (let i = 0; i < hints.length; i++) {
          if (hay.indexOf(hints[i]) >= 0) {
            rank = i;
            break;
          }
        }
        // For the role case we require a role match; for the sample case we keep
        // everything (Rust decides by coverage) but still order matched fonts first.
        if (!hasSample && rank > hints.length) {
          continue;
        }
        seen.add(info.path);
        cands.push({
          path: info.path,
          rank: rank,
          weight: (info.weight ?? 400) as number,
          italic: (info.italic ?? false) as boolean,
        });
      }

      cands.sort((a, b) => {
        if (a.rank != b.rank) {
          return a.rank - b.rank;
        }
        let sa: number = (a.italic == wantItalic) ? 0 : 10000;
        let sb: number = (b.italic == wantItalic) ? 0 : 10000;
        let pa: number = sa + Math.abs(a.weight - wantWeight);
        let pb: number = sb + Math.abs(b.weight - wantWeight);
        return pa - pb;
      });

      if (cands.length == 0) {
        return "";
      }
      // Role case: a single best path is enough. Sample case: hand Rust the whole
      // ordered list so it can pick the first that actually covers the glyphs.
      let out: string[] = [];
      if (hasSample) {
        for (let c of cands) {
          out.push(c.path);
        }
      } else {
        out.push(cands[0].path);
      }
      return out.join("\n");
    } catch (e) {
      console.error("getSystemFontPaths failed: " + JSON.stringify(e));
      return "";
    }
  }

  // Role → substring hints (normalized, best first), matched against a font's
  // family/fullName/family-list name. Mirrors the heuristics the Rust side used to run
  // over filenames; kept here so font selection lives next to the ArkTS font API.
  private static roleHints(role: string, lang: string): string[] {
    if (role == "serif") {
      return ["harmonyossansserif", "notoserif", "serif"];
    }
    if (role == "mono") {
      return ["harmonyossansmono", "monospace", "mono"];
    }
    if (role == "emoji") {
      return ["hmoscoloremoji", "coloremoji", "emoji", "notocoloremoji"];
    }
    if (role == "cjk") {
      if (lang.startsWith("ja")) {
        return ["notosansjp", "harmonyossansjp", "jp", "cjk", "notosanscjk"];
      }
      if (lang.startsWith("ko")) {
        return ["notosanskr", "harmonyossanskr", "kr", "cjk", "notosanscjk"];
      }
      if (lang.startsWith("zh-hant") || lang.startsWith("zh-tw")
        || lang.startsWith("zh-hk") || lang.indexOf("hant") >= 0) {
        return ["harmonyossanstc", "notosanstc", "tc", "cjk", "notosanscjk"];
      }
      return ["harmonyossanssc", "notosanssc", "sc", "cjk", "notosanscjk"];
    }
    // "ui" and anything else.
    return ["harmonyossans", "roboto", "sans"];
  }

  private static normalizeFontId(s: string): string {
    return s.toLowerCase().replace(/[_\- ]/g, "");
  }

  private async showTextInput(): Promise<void> {
    this.listenKeyboardHeightChange();
    await this.attach(true);
      if (this.imcFlag != true) {
      this.listenKeyBoardEvent();
    }
    this.inputMethodController.showTextInput().then(() => {
      console.debug("Succeeded in showing softKeyboard");
    }).catch((err: Any) => {
      console.error("Failed to show softKeyboard:" + JSON.stringify(err));
    });
  }

  private async hideTextInput(): Promise<void> {
    this.inputMethodController.hideTextInput().then(() => {
      console.debug("Succeeded in hide softKeyboard");
    }).catch((err: Any) => {
      console.error("Failed to hide softKeyboard:" + JSON.stringify(err));
    })
    this.cancelKeyboardHeightChange();
  }

  private async attach(showKeyboard: boolean): Promise<void> {
    try {
      await this.inputMethodController.attach(showKeyboard, this.textConfig);
    } catch (err) {
      console.error("Failed to attach:" + JSON.stringify(err));
    }
  }

  private listenKeyBoardEvent(): void {
    try {
      this.inputMethodController.on('insertText', (text) => {
      this.xComponentContext?.handleInsertTextEvent(text);
      console.debug("insertText: " + text);
    });
    } catch (err) {
      console.error("Failed to subscribe insertText:" + JSON.stringify(err));
      this.cancelListenKeyBoardEvent();
      return;
    }

    try {
      this.inputMethodController.on('deleteLeft', (length) => {
        this.xComponentContext?.handleDeleteLeftEvent(length);
        console.debug("deleteLeft, length:" + JSON.stringify(length));
      })
    } catch (err) {
      console.error("Failed to subscribe deleteLeft:" + JSON.stringify(err));
      this.cancelListenKeyBoardEvent();
      return;
    }

    try {
      this.inputMethodController.on('sendKeyboardStatus', (state) => {
        if (state == inputMethod.KeyboardStatus.HIDE) {
          this.xComponentContext?.handleKeyboardStatus(false, 0);
          this.inputMethodController.detach();
          this.cancelListenKeyBoardEvent();
        }
        console.debug("sendKeyboardStatus, state:" + JSON.stringify(state));
      })
    } catch (err) {
      console.error("Failed to subscribe sendKeyboardStatus:" + JSON.stringify(err));
      this.cancelListenKeyBoardEvent();
      return;
    }

    console.debug("listenKeyBoardEvent success");
    this.imcFlag = true;
  }

  private cancelListenKeyBoardEvent(): void {
    this.inputMethodController.off('insertText');
    this.inputMethodController.off('deleteLeft');
    this.inputMethodController.off('sendKeyboardStatus');
    this.imcFlag = false;
  }

  private listenKeyboardHeightChange() {
    window.getLastWindow(getContext(this)).then(currentWindow => {
      try {
        currentWindow.on('keyboardHeightChange', (data) => {
          console.info('Succeeded in enabling the listener for keyboard height changes. Data: ' + JSON.stringify(data));
          if (data != 0) {
            this.xComponentContext?.handleKeyboardStatus(true,data);
          }
        });
      } catch (exception) {
        console.error(`Failed to enable the listener for keyboard height changes. Cause code: ${exception.code}, message: ${exception.message}`);
      }
    })
  }

  private cancelKeyboardHeightChange() {
    window.getLastWindow(getContext(this)).then(currentWindow => {
      try {
        currentWindow.off('keyboardHeightChange');
      } catch (exception) {
        console.error(`Failed to enable the listener for keyboard height changes. Cause code: ${exception.code}, message: ${exception.message}`);
      }
    })
  }

}

export interface XComponentAttrs {
  id: string;
  type: number;
  libraryname: string;
}