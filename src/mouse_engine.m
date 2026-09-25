#import <Foundation/Foundation.h>
#import <CoreFoundation/CoreFoundation.h>

// The helper owns all mouse transformations. This only sets the current input
// owner, using its local message protocol. Never called from a CGEvent callback.
int lan_mouse_engine_update(int mode) {
    @autoreleasepool {
        CFMessagePortRef port = CFMessagePortCreateRemote(NULL, CFSTR("com.nuebling.mac-mouse-fix.helper"));
        if (!port) return 0;
        NSDictionary *request = @{
            @"message": @"lanMouseRemoteInput",
            @"payload": @{@"version": @1, @"active": @(mode == 2), @"sending": @(mode == 1), @"buttons": @32}
        };
        NSError *error = nil;
        NSData *data = [NSKeyedArchiver archivedDataWithRootObject:request requiringSecureCoding:NO error:&error];
        CFDataRef reply = NULL;
        SInt32 status = data ? CFMessagePortSendRequest(port, 0x420666, (__bridge CFDataRef)data,
            0.2, 0.2, kCFRunLoopDefaultMode, &reply) : -1;
        CFRelease(port);
        if (status != kCFMessagePortSuccess || !reply) { if (reply) CFRelease(reply); return 0; }
        NSData *responseData = CFBridgingRelease(reply);
        if (responseData.length > 4096) return 0;
        NSSet *classes = [NSSet setWithObjects:NSDictionary.class, NSMutableDictionary.class,
            NSString.class, NSNumber.class, nil];
        id response = [NSKeyedUnarchiver unarchivedObjectOfClasses:classes fromData:responseData error:&error];
        return [response isKindOfClass:NSDictionary.class]
            && [response[@"version"] isEqual:@1]
            && [response[@"accepted"] isEqual:@YES]
            && [response[@"rawCapture"] isEqual:@YES];
    }
}
