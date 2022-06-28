export class ToWasmMsg {
    constructor(app) {
        this.app = app
        this.ptr = app.new_wasm_msg_with_u64_capacity(1024);
        this.u32_ptr = this.ptr >> 2;
        this.u32_offset = this.u32_ptr + 2;

        this.u32_capacity = app.u32[this.u32_ptr] << 1;
    }
    
    reserve_u32(u32_capacity) {
        let app = this.app;

        this.u32_capacity += u32_capacity;
        let u64_capacity_needed = (this.u32_capacity & 1 + this.u32_capacity) >> 1;
        let offset = this.u32_offset - this.u32_ptr;
        let u64_len = (offset & 1 + offset) >> 1;
        
        if (app.u32[this.u32_ptr] - u64_len < u64_capacity_needed) {
            app.u32[this.u32_ptr + 1] = u64_len;
            this.ptr = this.app.wasm_msg_reserve_u64(this.ptr, u64_capacity_needed);
            this.u32_ptr = this.ptr >> 2;
            this.u32_offset = this.u32_ptr + offset;
        }
    }
    
    finalise(){
        let app = this.app;
        let ptr = this.ptr;
        let offset = this.u32_offset - this.u32_ptr;
        
        if(offset&1 != 0){ 
            app.u32[this.u32_offset+ 1] = 0
        }

        let u64_len = (offset & 1 + offset) >> 1;
        app.u32[this.u32_ptr + 1] = u64_len;
        /*
        for(let i = 0;i < offset;i++){
            console.log(i," - ",app.u32[i + this.u32_ptr]);
        }*/
        
        this.app = null;
        this.ptr = 0;
        this.u32_ptr = 0;
        this.u32_offset = 0;
        this.u32_capacity = 0;
        
        return ptr;
    }
    
    push_str(str){
        let app = this.app;
        this.reserve_u32(str.length + 1);
        app.u32[this.u32_offset ++] = str.length;
        for (let i = 0; i < str.length; i ++) {
            this.u32[this.u32_offset ++] = str.charCodeAt(i)
        }
    }
}

export class FromWasmMsg {
    constructor(app, ptr) {
        this.app = app
        this.ptr = ptr;
        this.u32_ptr = this.ptr >> 2;
        this.u32_offset = this.u32_ptr + 2;
    }
    
    destroy(){
        let app = this.app;
        app.wasm_msg_free(this.ptr);
        this.app = null;
        this.ptr = 0;
        this.u32_ptr = 0;
        this.u32_offset = 0;
    }
    
    read_str(){
        let app = this.app;
        let len = app.u32[this.u32_offset++];
        let str = "";
        for(let i = 0; i < len; i++){
            str += String.fromCharCode(app.u32[this.u32_offset++]);
        }
        return str
    }
}
