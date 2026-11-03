#import <CoreFoundation/CoreFoundation.h>

// These are the per-host preferences used by Displays > Advanced.
// -1 means no explicit preference; -2 reports a read/sync failure.
int lan_mouse_universal_control_read(void) {
    CFStringRef domain = CFSTR("com.apple.universalcontrol");
    if (!CFPreferencesSynchronize(domain, kCFPreferencesCurrentUser, kCFPreferencesCurrentHost)) return -2;
    CFPropertyListRef value = CFPreferencesCopyValue(CFSTR("Disable"), domain,
        kCFPreferencesCurrentUser, kCFPreferencesCurrentHost);
    if (!value) return -1;
    int result = -2;
    if (CFGetTypeID(value) == CFBooleanGetTypeID()) {
        result = CFBooleanGetValue(value) ? 1 : 0;
    } else if (CFGetTypeID(value) == CFNumberGetTypeID()) {
        int number;
        if (CFNumberGetValue(value, kCFNumberIntType, &number) && (number == 0 || number == 1)) result = number;
    }
    CFRelease(value);
    return result;
}

int lan_mouse_universal_control_write(int disabled) {
    CFStringRef domain = CFSTR("com.apple.universalcontrol");
    CFPreferencesSetValue(CFSTR("Disable"), disabled < 0 ? NULL :
        (disabled ? kCFBooleanTrue : kCFBooleanFalse), domain,
        kCFPreferencesCurrentUser, kCFPreferencesCurrentHost);
    return CFPreferencesSynchronize(domain, kCFPreferencesCurrentUser, kCFPreferencesCurrentHost);
}
