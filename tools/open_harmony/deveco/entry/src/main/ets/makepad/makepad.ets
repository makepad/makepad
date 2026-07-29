import display from '@ohos.display';
import deviceInfo from '@ohos.deviceInfo';
import inputMethod from '@ohos.inputMethod';
import picker from '@ohos.file.picker';
import { window } from '@kit.ArkUI';
import common from '@ohos.app.ability.common';
import resourceManager from '@ohos.resourceManager';
import makepadNative from 'libmakepad.so';

declare type Any = ESObject;

export interface MakepadXComponentInterface{
  onCreate(ark_ts: object):void;
  handleInsertTextEvent(text:String):void;
  handleDeleteLeftEvent(length: number):void;
  handleKeyboardStatus(isOpen:boolean, keyboardHeight:number):void;
  handleFileDialogResult(paths: Array<string>, cancelled: boolean, unsupported: boolean):void;
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

function parseAcceptFilters(accept: string): Array<string> {
  if (!accept || accept.length === 0) {
    return [];
  }
  let parts: Array<string> = [];
  accept.split(',').forEach((item: string) => {
    let trimmed = item.trim();
    if (trimmed.length > 0) {
      parts.push(trimmed);
    }
  });
  return parts;
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

  private emitFileDialogResult(paths: Array<string>, cancelled: boolean, unsupported: boolean): void {
    try {
      if (this.xComponentContext) {
        this.xComponentContext.handleFileDialogResult(paths, cancelled, unsupported);
      } else {
        // Fallback when the XComponent context is not yet bound.
        makepadNative.handleFileDialogResult(paths, cancelled, unsupported);
      }
    } catch (err) {
      console.error("handleFileDialogResult failed:" + JSON.stringify(err));
    }
  }

  openFileDialog(accept: string): void {
    try {
      let documentViewPicker = new picker.DocumentViewPicker(this.context);
      let options = new picker.DocumentSelectOptions();
      options.maxSelectNumber = 1;
      let filters = parseAcceptFilters(accept);
      if (filters.length > 0) {
        options.fileSuffixFilters = filters;
      }
      documentViewPicker.select(options).then((uris: Array<string>) => {
        if (!uris || uris.length === 0) {
          this.emitFileDialogResult([], true, false);
        } else {
          this.emitFileDialogResult(uris, false, false);
        }
      }).catch((err: Any) => {
        console.info("openFileDialog cancelled/failed:" + JSON.stringify(err));
        this.emitFileDialogResult([], true, false);
      });
    } catch (err) {
      console.error("openFileDialog unsupported:" + JSON.stringify(err));
      this.emitFileDialogResult([], false, true);
    }
  }

  saveFileDialog(filename: string, accept: string): void {
    try {
      let documentViewPicker = new picker.DocumentViewPicker(this.context);
      let options = new picker.DocumentSaveOptions();
      options.newFileNames = [filename && filename.length > 0 ? filename : 'untitled'];
      let filters = parseAcceptFilters(accept);
      if (filters.length > 0) {
        // strip leading dots for fileSuffixChoices when present
        options.fileSuffixChoices = filters.map((f: string) => f.startsWith('.') ? f.substring(1) : f);
      }
      documentViewPicker.save(options).then((uris: Array<string>) => {
        if (!uris || uris.length === 0) {
          this.emitFileDialogResult([], true, false);
        } else {
          this.emitFileDialogResult(uris, false, false);
        }
      }).catch((err: Any) => {
        console.info("saveFileDialog cancelled/failed:" + JSON.stringify(err));
        this.emitFileDialogResult([], true, false);
      });
    } catch (err) {
      console.error("saveFileDialog unsupported:" + JSON.stringify(err));
      this.emitFileDialogResult([], false, true);
    }
  }

  openFolderDialog(): void {
    try {
      let documentViewPicker = new picker.DocumentViewPicker(this.context);
      let options = new picker.DocumentSelectOptions();
      options.maxSelectNumber = 1;
      // API 12+: DocumentSelectMode.FOLDER. Fall back to unsupported if missing.
      let mode = (picker as Any).DocumentSelectMode;
      if (mode === undefined || mode.FOLDER === undefined) {
        this.emitFileDialogResult([], false, true);
        return;
      }
      options.selectMode = mode.FOLDER;
      documentViewPicker.select(options).then((uris: Array<string>) => {
        if (!uris || uris.length === 0) {
          this.emitFileDialogResult([], true, false);
        } else {
          this.emitFileDialogResult(uris, false, false);
        }
      }).catch((err: Any) => {
        console.info("openFolderDialog cancelled/failed:" + JSON.stringify(err));
        this.emitFileDialogResult([], true, false);
      });
    } catch (err) {
      console.error("openFolderDialog unsupported:" + JSON.stringify(err));
      this.emitFileDialogResult([], false, true);
    }
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
