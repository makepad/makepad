import {ArkGlue, MakepadXComponentInterface, XComponentAttrs} from '../makepad/makepad';

@Entry
@Component
struct Index {
  xComponentContext: MakepadXComponentInterface | undefined = undefined;
  xComponentAttrs: XComponentAttrs = {
    id: 'Entry',
    type: XComponentType.SURFACE,
    libraryname: 'entry',
  }

  build() {
    Row() {
      Column() {
        XComponent(this.xComponentAttrs)
          .focusable(true)
          .onLoad((xComponentContext) => {
            this.xComponentContext = xComponentContext as MakepadXComponentInterface;
            ArkGlue.getInstance().xComponentContext = this.xComponentContext;
            //this.xComponentContext.initMakepad(ArkGlue.getInstance());
          })
      }
      .width('100%')
    }
    .height('100%')
  }
}