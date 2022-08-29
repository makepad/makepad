#import <Foundation/Foundation.h>

@protocol MetalXPCProtocol
-(void)fetchTexture:(NSUInteger)index uid:(NSUInteger)uid with:(void (^)(NSObject*, NSUInteger))completion;
-(void)storeTexture:(NSUInteger)index obj:(NSObject*)obj uid:(NSUInteger)uid with:(void (^)())completion;;
@end

Protocol* define_xpc_service_protocol(void){
    return @protocol(MetalXPCProtocol);
}

NSObject* get_fetch_texture_completion_block(void(^block)(NSObject*, NSUInteger)){
    return Block_copy(^(NSObject * texture, NSUInteger uid){
        block(texture, uid); 
    });
}

NSObject* get_store_completion_block(void(^block)()){
    return Block_copy(^(){
        block();
    });
}
