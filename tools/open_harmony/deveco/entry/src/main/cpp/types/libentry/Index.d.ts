export const onCreate: (ark_ts: object) => void;
export const handleInsertTextEvent: (text:String) => void;
export const handleDeleteLeftEvent: (length: number) => void
export const handleKeyboardStatus: (isOpen:boolean, keyboardHeight:number) => void;
export const handleFileDialogResult: (paths: Array<string>, cancelled: boolean, unsupported: boolean) => void;
