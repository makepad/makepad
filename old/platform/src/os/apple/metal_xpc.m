#import <Foundation/Foundation.h>
#include <stdint.h>

@protocol MetalXPCProtocol
-(void)fetchTexture:(uint64_t)presentable_image_id_u64 _padding:(uint64_t)_padding with:(void (^)(NSObject*, /*padding*/uint64_t))completion;
-(void)storeTexture:(uint64_t)presentable_image_id_u64 obj:(NSObject*)obj _padding:(uint64_t)_padding with:(void (^)())completion;
@end

Protocol* define_xpc_service_protocol(void) {
    return @protocol(MetalXPCProtocol);
}

NSObject* hackily_heapify_block2_obj_u64(void(^block)(NSObject*, /*padding*/uint64_t)){
    return Block_copy(^(NSObject *texture, uint64_t _padding) {
        block(texture, _padding);
    });
}

NSObject* hackily_heapify_block0(void(^block)()) {
    return Block_copy(^() {
        block();
    });
}
