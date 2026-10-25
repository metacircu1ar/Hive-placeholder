/* Compile the product adapter against inert Security call substitutes. */
#define SecItemCopyMatching fixture_copy_matching
#define SecItemDelete fixture_delete
#include "../../src/os/keychain.m"
#include <assert.h>

static uint8_t item_id = 1;
static bool present = true;
static bool changed_account = false;
static unsigned deletes = 0;

static bool reference_matches(CFDictionaryRef q) {
    CFArrayRef items = CFDictionaryGetValue(q, kSecMatchItemList);
    assert(items && CFGetTypeID(items) == CFArrayGetTypeID());
    assert(CFArrayGetCount(items) == 1);
    CFDataRef ref = CFArrayGetValueAtIndex(items, 0);
    assert(CFGetTypeID(ref) == CFDataGetTypeID());
    return CFDataGetLength(ref) == 1 && CFDataGetBytePtr(ref)[0] == item_id;
}
static bool attributes_match(CFDictionaryRef q) {
    CFTypeRef service = CFDictionaryGetValue(q, kSecAttrService);
    CFTypeRef account = CFDictionaryGetValue(q, kSecAttrAccount);
    return service && account && CFEqual(service, CFSTR("service")) &&
        CFEqual(account, changed_account ? CFSTR("other") : CFSTR("account"));
}
static void safe_query(CFDictionaryRef q) {
    assert(!CFDictionaryContainsKey(q, kSecReturnData));
    assert(!CFDictionaryContainsKey(q, kSecValueData));
    LAContext *context = (LAContext *)CFDictionaryGetValue(q, kSecUseAuthenticationContext);
    assert(context && context.interactionNotAllowed);
}
OSStatus fixture_copy_matching(CFDictionaryRef q, CFTypeRef *result) {
    safe_query(q);
    if (!present) return errSecItemNotFound;
    if (!result) {
        assert(!CFDictionaryContainsKey(q, kSecAttrAccount));
        return reference_matches(q) ? errSecSuccess : errSecItemNotFound;
    }
    if (!attributes_match(q)) return errSecItemNotFound;
    assert(CFDictionaryGetValue(q, kSecReturnAttributes) == kCFBooleanTrue);
    assert(CFDictionaryGetValue(q, kSecReturnPersistentRef) == kCFBooleanTrue);
    CFDataRef ref = CFDataCreate(NULL, &item_id, 1);
    const void *keys[] = {kSecAttrService, kSecAttrAccount, kSecValuePersistentRef};
    const void *values[] = {CFSTR("service"), CFSTR("account"), ref};
    *result = CFDictionaryCreate(NULL, keys, values, 3,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    CFRelease(ref);
    return errSecSuccess;
}
OSStatus fixture_delete(CFDictionaryRef q) {
    safe_query(q);
    assert(!CFDictionaryContainsKey(q, kSecValuePersistentRef));
    if (!present || !attributes_match(q) || !reference_matches(q)) return errSecItemNotFound;
    present = false;
    deletes++;
    return errSecSuccess;
}
int main(void) {
    uint8_t bytes[16] = {0};
    size_t length = 0;
    assert(hive_keychain_reference("service", "account", bytes, sizeof(bytes), &length) == 0);
    assert(length == 1 && bytes[0] == 1);
    assert(hive_keychain_reference("service", "wrong", bytes, sizeof(bytes), &length) == errSecItemNotFound);
    assert(hive_keychain_exists(bytes, length) == 0);
    item_id = 2;
    assert(hive_keychain_delete("service", "account", bytes, length) == errSecItemNotFound);
    assert(deletes == 0 && present);
    item_id = 1;
    changed_account = true;
    assert(hive_keychain_delete("service", "account", bytes, length) == errSecItemNotFound);
    assert(hive_keychain_exists(bytes, length) == 0);
    assert(deletes == 0 && present);
    changed_account = false;
    assert(hive_keychain_delete("service", "account", bytes, length) == 0);
    assert(deletes == 1 && !present);
    assert(hive_keychain_exists(bytes, length) == errSecItemNotFound);
    return 0;
}
