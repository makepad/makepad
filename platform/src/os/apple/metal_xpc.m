#import <Foundation/Foundation.h>

@protocol MetalXPCProtocol
-(void)fetchTexture:(NSUInteger)index with:(void (^)(NSObject*))completion;
-(void)storeTexture:(NSUInteger)index obj:(NSObject*)obj;
@end

Protocol* define_xpc_service_protocol(void){
    return @protocol(MetalXPCProtocol);
}

NSObject* get_xpc_completion_block(){
    return ^(NSObject * texture){
        NSLog(@"IT WORKED");
    };
}