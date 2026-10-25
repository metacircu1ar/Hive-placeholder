/* Credential metadata only: never request kSecReturnData or password bytes. */
#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>
#import <LocalAuthentication/LocalAuthentication.h>
#include <stdint.h>
#include <string.h>

static CFMutableDictionaryRef query(const char *service, const char *account) {
    CFMutableDictionaryRef q = CFDictionaryCreateMutable(NULL, 0,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (!q) return NULL;
    if (service && account) {
        CFStringRef s = CFStringCreateWithCString(NULL, service, kCFStringEncodingUTF8);
        CFStringRef a = CFStringCreateWithCString(NULL, account, kCFStringEncodingUTF8);
        if (!s || !a) {
            if (s) CFRelease(s);
            if (a) CFRelease(a);
            CFRelease(q);
            return NULL;
        }
        CFDictionarySetValue(q, kSecAttrService, s);
        CFDictionarySetValue(q, kSecAttrAccount, a);
        CFRelease(s);
        CFRelease(a);
    }
    CFDictionarySetValue(q, kSecClass, kSecClassGenericPassword);
    LAContext *context = [[LAContext alloc] init];
    if (!context) { CFRelease(q); return NULL; }
    context.interactionNotAllowed = YES;
    CFDictionarySetValue(q, kSecUseAuthenticationContext, (CFTypeRef)context);
    [context release];
    return q;
}

static int32_t reference(const char *service, const char *account,
                              uint8_t *bytes, size_t capacity, size_t *length) {
    CFMutableDictionaryRef q = query(service, account);
    if (!q) return errSecAllocate;
    CFDictionarySetValue(q, kSecMatchLimit, kSecMatchLimitOne);
    CFDictionarySetValue(q, kSecReturnAttributes, kCFBooleanTrue);
    CFDictionarySetValue(q, kSecReturnPersistentRef, kCFBooleanTrue);
    CFTypeRef found = NULL;
    OSStatus status = SecItemCopyMatching(q, &found);
    if (status == errSecSuccess) {
        if (!found || CFGetTypeID(found) != CFDictionaryGetTypeID()) {
            status = errSecDecode;
        } else {
            CFDictionaryRef row = (CFDictionaryRef)found;
            CFTypeRef s = CFDictionaryGetValue(row, kSecAttrService);
            CFTypeRef a = CFDictionaryGetValue(row, kSecAttrAccount);
            CFTypeRef ref = CFDictionaryGetValue(row, kSecValuePersistentRef);
            if (!s || !a || !ref || !CFEqual(s, CFDictionaryGetValue(q, kSecAttrService)) ||
                !CFEqual(a, CFDictionaryGetValue(q, kSecAttrAccount)) ||
                CFGetTypeID(ref) != CFDataGetTypeID()) {
                status = errSecDecode;
            } else {
                CFIndex n = CFDataGetLength((CFDataRef)ref);
                if (n <= 0 || (size_t)n > capacity) status = errSecDecode;
                else {
                    memcpy(bytes, CFDataGetBytePtr((CFDataRef)ref), (size_t)n);
                    *length = (size_t)n;
                }
            }
        }
    }
    if (found) CFRelease(found);
    CFRelease(q);
    return status;
}

static OSStatus match_reference(CFMutableDictionaryRef q, const uint8_t *bytes, size_t length) {
    CFDataRef ref = CFDataCreate(NULL, bytes, (CFIndex)length);
    if (!ref) return errSecAllocate;
    const void *values[] = {ref};
    CFArrayRef items = CFArrayCreate(NULL, values, 1, &kCFTypeArrayCallBacks);
    CFRelease(ref);
    if (!items) return errSecAllocate;
    CFDictionarySetValue(q, kSecMatchItemList, items);
    CFRelease(items);
    return errSecSuccess;
}

static int32_t delete_item(const char *service, const char *account,
                           const uint8_t *bytes, size_t length) {
    CFMutableDictionaryRef q = query(service, account);
    if (!q) return errSecAllocate;
    OSStatus status = match_reference(q, bytes, length);
    if (status != errSecSuccess) { CFRelease(q); return status; }
    status = SecItemDelete(q);
    CFRelease(q);
    return status;
}

int32_t hive_keychain_reference(const char *service, const char *account,
                              uint8_t *bytes, size_t capacity, size_t *length) {
    @autoreleasepool { return reference(service, account, bytes, capacity, length); }
}
int32_t hive_keychain_delete(const char *service, const char *account,
                           const uint8_t *bytes, size_t length) {
    @autoreleasepool { return delete_item(service, account, bytes, length); }
}

int32_t hive_keychain_exists(const uint8_t *bytes, size_t length) {
    @autoreleasepool {
        CFMutableDictionaryRef q = query(NULL, NULL);
        if (!q) return errSecAllocate;
        OSStatus status = match_reference(q, bytes, length);
        if (status == errSecSuccess) {
            CFDictionarySetValue(q, kSecMatchLimit, kSecMatchLimitOne);
            status = SecItemCopyMatching(q, NULL);
        }
        CFRelease(q);
        return status;
    }
}
