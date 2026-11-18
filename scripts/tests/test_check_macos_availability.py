# SPDX-License-Identifier: GPL-3.0-or-later
"""Unit tests for scripts/check-macos-availability.py (header parser, tool-output parsers,
matcher, both sub-commands). Standard library only; runs on Windows and macOS:

    python3 -I scripts/tests/test_check_macos_availability.py -v

The header snippets below are written for these tests in the style of Apple's SDK headers
(the API names are real, the text is not copied from the SDK)."""
import importlib.util
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location("cma", os.path.join(HERE, "..", "check-macos-availability.py"))
cma = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cma)

SCK_LIKE = r"""
//
//  SCStream.h  (test snippet in the style of ScreenCaptureKit)
//  API_AVAILABLE(macos(99.0)) in a comment must be ignored
//
#import <Foundation/Foundation.h>
#if __has_include(<Foo/Foo.h>) \
    && API_AVAILABLE(macos(98.0))
#endif

NS_ASSUME_NONNULL_BEGIN

@class SCContentFilter;
@protocol SCStreamDelegate, SCStreamOutput;

API_AVAILABLE(macos(12.3)) API_UNAVAILABLE(ios, tvos, watchos)
@interface SCStreamConfiguration : NSObject
@property (nonatomic, assign) size_t width;
@property (nonatomic, assign) BOOL capturesAudio API_AVAILABLE(macos(13.0));
@property (nonatomic, assign) BOOL captureMicrophone API_AVAILABLE(macos(15.0));
@property (nonatomic, readonly, getter=isSomethingNew) BOOL somethingNew API_AVAILABLE(macos(14.0));
@property (nonatomic, copy, setter=setHandlerCustom:) void (^handler)(NSError * _Nullable error) API_AVAILABLE(macos(14.2));
- (instancetype)initWithPreset:(NSInteger)preset options:(NSDictionary *)options API_AVAILABLE(macos(15.0));
+ (instancetype)streamConfigurationWithPreset:(NSInteger)preset NS_SWIFT_NAME(init(preset:)) API_AVAILABLE(macos(15.0));
- (void)plainOldMethod;
@end

NS_SWIFT_SENDABLE
API_AVAILABLE(macos(14.0))
@interface SCContentSharingPicker : NSObject
- (void)present;
- (void)presentPickerUsingContentStyle:(NSInteger)contentStyle
                                     API_AVAILABLE(macos(14.0));
@property (class, readonly) SCContentSharingPicker *sharedPicker;
@end

@interface NSObject (SCTestCategory)
- (void)categoryMethodNew API_AVAILABLE(macos(14.4));
@end

@protocol SCStreamDelegate <NSObject>
@optional
- (void)stream:(id)stream didStopWithError:(NSError *)error;
- (void)streamDidBecomeActive:(id)stream API_AVAILABLE(macos(15.2));
@end

NS_CLASS_AVAILABLE(10_15, 13_0)
@interface OldStyleClass : NSObject
@end

API_AVAILABLE_BEGIN(macos(14.0))
@interface RegionClass : NSObject
- (void)regionMethod;
@end
extern NSString * const SCRegionConstant;
API_AVAILABLE_END

typedef NS_ENUM(NSInteger, SCStreamType) {
    SCStreamTypeWindow,
    SCStreamTypeDisplay API_AVAILABLE(macos(14.0)),
} API_AVAILABLE(macos(12.3));

NS_ASSUME_NONNULL_END
"""

C_LIKE = """
/* AudioHardware.h (test snippet in the style of CoreAudio) */
#ifdef __cplusplus\r
extern "C" {\r
#endif\r
CF_ASSUME_NONNULL_BEGIN

extern OSStatus
AudioObjectGetPropertyData(AudioObjectID inObjectID, const AudioObjectPropertyAddress* inAddress,
                           UInt32 inQualifierDataSize, const void* __nullable inQualifierData,
                           UInt32* ioDataSize, void* outData) API_AVAILABLE(macos(10.4));

extern OSStatus
AudioHardwareCreateProcessTap(CATapDescription* __nullable inDescription,
                              AudioObjectID* __nullable outTapID)
    __attribute__((weak_import)) API_AVAILABLE(macos(14.2)) API_UNAVAILABLE(ios, watchos, tvos);

API_AVAILABLE(macos(13.1))
CF_EXPORT CFStringRef SomeLeadingAnnotatedFunction(void) CF_RETURNS_RETAINED;

extern OSStatus AudioHardwareUnload(void) __OSX_AVAILABLE_STARTING(__MAC_10_1, __IPHONE_2_0);
extern OSStatus OldNoAnnotation(UInt32 x);
extern void SomeAttrFunction(int) __attribute__((availability(macos,introduced=14.0)));

extern const CFStringRef kAudioTestNewKey API_AVAILABLE(macos(14.0));
CF_EXPORT const CFStringRef kAudioTestOldKey;
extern void (*gTestCallbackPointer)(int) API_AVAILABLE(macos(14.0));
extern const unsigned char TestVersionString[] API_AVAILABLE(macos(15.0));

CF_INLINE Float64 InlineHelper(Float64 x) API_AVAILABLE(macos(14.0)) { return x * 2.0; }
static inline int StaticHelper(int x) { return x; }
extern OSStatus AfterInlineFunction(void) API_AVAILABLE(macos(14.1));

typedef OSStatus (*TestProc)(int) API_AVAILABLE(macos(14.0));
typedef struct TestStruct { int a; float b; } TestStruct API_AVAILABLE(macos(14.0));

extern OSStatus AudioDeprecatedThing(void) API_DEPRECATED("use other", macos(10.0, 14.0));
extern OSStatus NSStyleFunction(void) NS_AVAILABLE_MAC(10_15);
extern OSStatus IOSOnlyFunction(void) NS_AVAILABLE_IOS(17_0);
extern OSStatus NotOnMac(void) __OSX_AVAILABLE_STARTING(__MAC_NA, __IPHONE_17_0);
extern OSStatus GenericDeprecated(void) __API_DEPRECATED_MSG("x", macos(10.0, 10.15));
extern _Float16 __fabsf16(_Float16) __API_AVAILABLE(macos(15.0), ios(18.0));
ZEXTERN uLong ZEXPORT crc32_combine_op OF((uLong crc1, uLong crc2, uLong op))
                                          __API_AVAILABLE(macos(14.0),ios(17.0));
extern int printf_like(const char *, ...) __printflike(1, 2) API_AVAILABLE(macos(14.0));
extern BOOL kTestBoolFlag API_AVAILABLE(macos(14.0));

CF_ASSUME_NONNULL_END
#ifdef __cplusplus
}
#endif
"""


def load_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def by(decls, kind):
    return {d["name"]: d["v"] for d in decls if d["kind"] == kind}


class Versions(unittest.TestCase):
    def test_parse_version(self):
        self.assertEqual(cma.parse_version("14.2"), (14, 2, 0))
        self.assertEqual(cma.parse_version("14_2"), (14, 2, 0))
        self.assertEqual(cma.parse_version("__MAC_10_15_4"), (10, 15, 4))
        self.assertEqual(cma.parse_version("13"), (13, 0, 0))
        self.assertIsNone(cma.parse_version("ios(17.0)"))
        self.assertIsNone(cma.parse_version("NA"))
        self.assertEqual(cma.vstr((13, 1, 0)), "13.1")
        self.assertEqual(cma.vstr((10, 12, 2)), "10.12.2")


class Annotations(unittest.TestCase):
    def v(self, text):
        return cma.annotations(text)[0]

    def test_forms(self):
        self.assertEqual(self.v("x API_AVAILABLE(macos(14.0), ios(17.0))"), (14, 0, 0))
        self.assertEqual(self.v("x API_AVAILABLE(macosx(13.1))"), (13, 1, 0))
        self.assertEqual(self.v("x __API_AVAILABLE(macos(15.0))"), (15, 0, 0))
        self.assertEqual(self.v('x API_DEPRECATED("m", macos(10.10, 14.0))'), (10, 10, 0))
        self.assertEqual(self.v("x __OSX_AVAILABLE_STARTING(__MAC_14_0, __IPHONE_17_0)"), (14, 0, 0))
        self.assertEqual(self.v("x __OSX_AVAILABLE(14.4)"), (14, 4, 0))
        self.assertEqual(self.v("x NS_AVAILABLE_MAC(14_0)"), (14, 0, 0))
        self.assertEqual(self.v("x NS_AVAILABLE(14_0, 17_0)"), (14, 0, 0))
        self.assertEqual(self.v("x NS_CLASS_AVAILABLE_MAC(10_15)"), (10, 15, 0))
        self.assertEqual(self.v("x CF_AVAILABLE(14_0, 17_0)"), (14, 0, 0))
        self.assertEqual(self.v("x CG_AVAILABLE_STARTING(14.0)"), (14, 0, 0))
        self.assertEqual(self.v("x __attribute__((availability(macosx, introduced=14.2)))"), (14, 2, 0))
        self.assertEqual(self.v("x MP_API(macos(10.12.2))"), (10, 12, 2))
        self.assertEqual(self.v("x AVAILABLE_MAC_OS_X_VERSION_10_5_AND_LATER"), (10, 5, 0))
        # takes the highest stated version
        self.assertEqual(self.v('x API_AVAILABLE(macos(14.0)) API_DEPRECATED("m", macos(14.0, 15.0))'), (14, 0, 0))

    def test_not_mac(self):
        for t in ("x NS_AVAILABLE_IOS(17_0)", "x API_AVAILABLE(ios(17.0))", "x API_UNAVAILABLE(macos)",
                  "x __OSX_AVAILABLE_STARTING(__MAC_NA, __IPHONE_17_0)", "x __IOS_AVAILABLE(17.0)",
                  "x API_AVAILABLE(macCatalyst(14.0))", "x NS_SWIFT_NAME(foo(bar:))", "x"):
            self.assertIsNone(self.v(t), t)

    def test_residual(self):
        _, r, names = cma.annotations("extern OSStatus F(int* _Nullable x) CF_RETURNS_RETAINED API_AVAILABLE(macos(14.0));")
        self.assertNotIn("API_AVAILABLE", r)
        self.assertNotIn("_Nullable", r)
        self.assertIn("F(", r)
        self.assertIn("CF_RETURNS_RETAINED", names)


class HeaderParser(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.objc = cma.parse_header(SCK_LIKE, "ScreenCaptureKit/SCStream.h")
        cls.c = cma.parse_header(C_LIKE, "CoreAudio/AudioHardware.h")

    def test_classes(self):
        c = by(self.objc, "class")
        self.assertEqual(c["SCStreamConfiguration"], (12, 3, 0))
        self.assertEqual(c["SCContentSharingPicker"], (14, 0, 0), "annotation two lines above @interface, after NS_SWIFT_SENDABLE")
        self.assertEqual(c["OldStyleClass"], (10, 15, 0))
        self.assertEqual(c["RegionClass"], (14, 0, 0), "API_AVAILABLE_BEGIN region")
        self.assertNotIn("SCContentFilter", c, "forward @class is not a declaration")
        self.assertNotIn("NSObject", c, "a category does not declare the class")
        self.assertNotIn("SCStreamDelegate", c, "protocols are not classes")
        self.assertNotIn("SCTestCategory", c)

    def test_selectors(self):
        s = {}
        for d in self.objc:
            if d["kind"] == "selector":
                s.setdefault(d["name"], []).append(d["v"])
        self.assertEqual(s["width"], [(12, 3, 0)], "inherits the class's version")
        self.assertEqual(s["setWidth:"], [(12, 3, 0)])
        self.assertEqual(s["capturesAudio"], [(13, 0, 0)])
        self.assertEqual(s["captureMicrophone"], [(15, 0, 0)])
        self.assertEqual(s["setCaptureMicrophone:"], [(15, 0, 0)])
        self.assertEqual(s["isSomethingNew"], [(14, 0, 0)], "getter= attribute")
        self.assertNotIn("setSomethingNew:", s, "readonly has no setter")
        self.assertEqual(s["handler"], [(14, 2, 0)], "block property name")
        self.assertEqual(s["setHandlerCustom:"], [(14, 2, 0)], "setter= attribute")
        self.assertEqual(s["initWithPreset:options:"], [(15, 0, 0)])
        self.assertEqual(s["streamConfigurationWithPreset:"], [(15, 0, 0)])
        self.assertEqual(s["plainOldMethod"], [(12, 3, 0)])
        self.assertEqual(s["present"], [(14, 0, 0)])
        self.assertEqual(s["presentPickerUsingContentStyle:"], [(14, 0, 0)], "annotation on the next line")
        self.assertEqual(s["sharedPicker"], [(14, 0, 0)])
        self.assertEqual(s["categoryMethodNew"], [(14, 4, 0)])
        self.assertEqual(s["stream:didStopWithError:"], [None], "protocol without annotation")
        self.assertEqual(s["streamDidBecomeActive:"], [(15, 2, 0)])
        self.assertEqual(s["regionMethod"], [(14, 0, 0)])

    def test_objc_header_constants_and_enums(self):
        sym = by(self.objc, "variable")
        self.assertEqual(sym["SCRegionConstant"], (14, 0, 0))
        self.assertNotIn("SCStreamTypeDisplay", sym, "enumerators are not symbols")
        self.assertNotIn("SCStreamType", sym)
        names = {d["name"] for d in self.objc}
        self.assertFalse(any(n.startswith("API_") for n in names))

    def test_c_functions(self):
        f = by(self.c, "function")
        self.assertEqual(f["AudioObjectGetPropertyData"], (10, 4, 0))
        self.assertEqual(f["AudioHardwareCreateProcessTap"], (14, 2, 0), "multi-line, weak_import + availability after the parameters")
        self.assertEqual(f["SomeLeadingAnnotatedFunction"], (13, 1, 0), "annotation on the line before")
        self.assertEqual(f["AudioHardwareUnload"], (10, 1, 0))
        self.assertIsNone(f["OldNoAnnotation"])
        self.assertEqual(f["SomeAttrFunction"], (14, 0, 0))
        self.assertEqual(f["AfterInlineFunction"], (14, 1, 0), "a declaration right after an inline body is still seen")
        self.assertEqual(f["AudioDeprecatedThing"], (10, 0, 0), "API_DEPRECATED: the first version is 'introduced'")
        self.assertEqual(f["NSStyleFunction"], (10, 15, 0))
        self.assertIsNone(f["IOSOnlyFunction"])
        self.assertIsNone(f["NotOnMac"])
        self.assertEqual(f["GenericDeprecated"], (10, 0, 0))
        self.assertEqual(f["__fabsf16"], (15, 0, 0), "reserved (double underscore) function name")
        self.assertEqual(f["printf_like"], (14, 0, 0), "__printflike() after the parameters is a macro")
        self.assertNotIn("__printflike", f)
        self.assertNotIn("OF", f, "OF((...)) is zlib's prototype macro")
        self.assertNotIn("InlineHelper", f)
        self.assertNotIn("StaticHelper", f)
        self.assertNotIn("TestProc", f)

    def test_c_variables(self):
        v = by(self.c, "variable")
        self.assertEqual(v["kAudioTestNewKey"], (14, 0, 0))
        self.assertIsNone(v["kAudioTestOldKey"], "CF_EXPORT counts as extern")
        self.assertEqual(v["gTestCallbackPointer"], (14, 0, 0))
        self.assertEqual(v["TestVersionString"], (15, 0, 0))
        self.assertEqual(v["crc32_combine_op"], (14, 0, 0), "zlib OF() style: name kept (kind does not matter)")
        self.assertEqual(v["kTestBoolFlag"], (14, 0, 0), "all-caps type removed with the macros")
        self.assertNotIn("TestStruct", v)
        self.assertNotIn("a", v)

    def test_line_numbers(self):
        d = [x for x in self.c if x["name"] == "AudioHardwareCreateProcessTap"][0]
        line = C_LIKE.replace("\r\n", "\n").split("\n").index("extern OSStatus", 12) + 1
        self.assertTrue(d["where"].startswith("CoreAudio/AudioHardware.h:"))
        self.assertGreaterEqual(int(d["where"].rsplit(":", 1)[1]), line - 1)

    def test_region_edges_and_all_caps_class(self):
        # The first declaration of a region starts (as text) before API_AVAILABLE_BEGIN, and
        # the first one after it starts before API_AVAILABLE_END: both must be judged by where
        # they end. An all-caps class name (NSURL style) must not be lost as a "macro".
        t = ("#include <x.h>\nNS_ASSUME_NONNULL_BEGIN\nAPI_AVAILABLE_BEGIN(macos(14.0))\n"
             "extern void RegionFirst(void);\nFOUNDATION_EXPORT NSString * const RegionConst;\n"
             "API_AVAILABLE_END\nextern void AfterRegionFn(void);\n"
             "API_AVAILABLE(macos(14.0))\n@interface NSNEWURL : NSObject\n- (void)allCapsClassMethod;\n@end\n"
             "NS_ASSUME_NONNULL_END\n")
        v = {d["name"]: d["v"] for d in cma.parse_header(t, "r.h")}
        self.assertEqual(v["RegionFirst"], (14, 0, 0))
        self.assertEqual(v["RegionConst"], (14, 0, 0))
        self.assertIsNone(v["AfterRegionFn"], "the region ended before it")
        self.assertEqual(v["NSNEWURL"], (14, 0, 0))
        self.assertEqual(v["allCapsClassMethod"], (14, 0, 0))


class Database(unittest.TestCase):
    def test_aggregation(self):
        a = "extern void Dup(void) API_AVAILABLE(macos(14.0));\nextern void Both(void) API_AVAILABLE(macos(15.0));\n" \
            "@interface A : NSObject\n- (void)sharedSel API_AVAILABLE(macos(14.0));\n- (void)onlyNew:(int)x API_AVAILABLE(macos(14.0));\n@end\n"
        b = "extern void Dup(void);\nextern void Both(void) API_AVAILABLE(macos(14.1));\n" \
            "@interface B : NSObject\n- (void)sharedSel;\n- (void)onlyNew:(int)x API_AVAILABLE(macos(15.0));\n@end\n"
        db = cma.build_db([(a, "a.h"), (b, "b.h")])
        self.assertNotIn("_Dup", db["symbols"], "one unannotated declaration: always available")
        self.assertEqual(db["symbols"]["_Both"]["v"], "14.1", "oldest declaration wins")
        self.assertNotIn("sharedSel", db["selectors"])
        self.assertEqual(db["selectors"]["onlyNew:"]["v"], "14.0")
        self.assertNotIn("A", db["classes"], "unannotated classes are always available")
        self.assertEqual(db["headers"], 2)

    def test_canaries(self):
        db = cma.build_db([(C_LIKE, "x.h"), (SCK_LIKE, "y.h")])
        self.assertEqual(cma.check_canaries(db, cma.DEFAULT_CANARIES), [])
        probs = cma.check_canaries(db, "symbol:_Missing=14.0,class:SCContentSharingPicker=15.0")
        self.assertEqual(len(probs), 2)
        self.assertIn("not found", probs[0])
        self.assertIn("expected 15.0", probs[1])


NM_OUT = """
/x/Jukeoke.app/Contents/MacOS/jukeoke (for architecture arm64):
                 (undefined) external _AudioObjectGetPropertyData (from CoreAudio)
                 (undefined) weak external _AudioHardwareCreateProcessTap (from CoreAudio)
                 (undefined) external _OBJC_CLASS_$_SCContentSharingPicker (from ScreenCaptureKit)
                 (undefined) external _OBJC_METACLASS_$_NSObject (from libobjc)
                 (undefined) external _kAudioTestNewKey (from CoreAudio)
                 (undefined [lazy bound]) external _objc_msgSend (from libobjc)
0000000100001000 (__TEXT,__text) external _main
"""

DYLD_OUT = """
/x/jukeoke [arm64]:
    -imports:
        0x0000  _AudioObjectGetPropertyData (from CoreAudio)
        0x0001  _AudioHardwareCreateProcessTap [weak-import] (from CoreAudio)
"""

SELREFS_OUT = """/x/jukeoke:
Contents of (__DATA,__objc_selrefs) section
0000000100010000  __TEXT:__objc_methname:alloc
0000000100010008  __TEXT:__objc_methname:initWithPreset:options:
0000000100010010  __TEXT:__objc_methname:present
"""

CSTRINGS_OUT = """/x/jukeoke:
Contents of (__TEXT,__cstring) section
0000000100020000  SCContentSharingPicker
0000000100020017  hello world
0000000100020030  streamDidBecomeActive:
"""


class ToolParsers(unittest.TestCase):
    def test_nm(self):
        r = cma.parse_nm_m(NM_OUT)
        self.assertIn(("_AudioObjectGetPropertyData", False, "CoreAudio"), r)
        self.assertIn(("_AudioHardwareCreateProcessTap", True, "CoreAudio"), r)
        self.assertIn(("_objc_msgSend", False, "libobjc"), r)
        self.assertNotIn("_main", [x[0] for x in r], "defined symbols are not imports")
        self.assertEqual(len(r), 6)

    def test_dyld_info(self):
        r = cma.parse_dyld_info_imports(DYLD_OUT)
        self.assertEqual(r, [("_AudioObjectGetPropertyData", False, "CoreAudio"),
                             ("_AudioHardwareCreateProcessTap", True, "CoreAudio")])
        self.assertEqual(cma.parse_dyld_info_imports("        0x0003  _objc_storeWeak (from libobjc)\n"),
                         [("_objc_storeWeak", False, "libobjc")], "'Weak' in the name is not a weak import")

    def test_otool(self):
        self.assertEqual(cma.parse_otool_selrefs(SELREFS_OUT), ["alloc", "initWithPreset:options:", "present"])
        self.assertEqual(cma.parse_otool_cstrings(CSTRINGS_OUT), ["SCContentSharingPicker", "hello world", "streamDidBecomeActive:"])


class Matcher(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = cma.build_db([(C_LIKE, "CoreAudio/AudioHardware.h"), (SCK_LIKE, "ScreenCaptureKit/SCStream.h")])

    def test_match(self):
        f = cma.match_file(self.db, (13, 0, 0), cma.parse_nm_m(NM_OUT), cma.parse_otool_selrefs(SELREFS_OUT),
                           cma.parse_otool_cstrings(CSTRINGS_OUT))
        got = {(x["verdict"], x["what"], x["name"]) for x in f}
        self.assertIn(("FAIL", "class", "_OBJC_CLASS_$_SCContentSharingPicker"), got)
        self.assertIn(("FAIL", "symbol", "_kAudioTestNewKey"), got)
        self.assertIn(("REVIEW", "symbol", "_AudioHardwareCreateProcessTap"), got, "weak import: review, not fail")
        self.assertIn(("REVIEW", "selector", "initWithPreset:options:"), got)
        self.assertIn(("REVIEW", "class name string", "SCContentSharingPicker"), got)
        self.assertIn(("REVIEW", "selector name string", "streamDidBecomeActive:"), got)
        self.assertNotIn("_AudioObjectGetPropertyData", {x[2] for x in got})
        self.assertNotIn("alloc", {x[2] for x in got})
        self.assertEqual(f[0]["verdict"], "FAIL", "FAIL lines first")

    def test_strong_new_function_fails(self):
        imports = [("_AudioHardwareCreateProcessTap", False, "CoreAudio")]
        f = cma.match_file(self.db, (13, 0, 0), imports, [], [])
        self.assertEqual([(x["verdict"], x["v"]) for x in f], [("FAIL", "14.2")])
        # the same binary is fine for a 14.2 minimum
        self.assertEqual(cma.match_file(self.db, (14, 2, 0), imports, [], []), [])

    def test_13_1_is_newer_than_13_0(self):
        f = cma.match_file(self.db, (13, 0, 0), [("_SomeLeadingAnnotatedFunction", False, None)], [], [])
        self.assertEqual(f[0]["verdict"], "FAIL")


class Commands(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def make_sdk(self, with_sck=True):
        sdk = os.path.join(self.dir, "MacOSX.sdk")
        ca = os.path.join(sdk, "System", "Library", "Frameworks", "CoreAudio.framework", "Versions", "A", "Headers")
        os.makedirs(ca)
        with open(os.path.join(ca, "AudioHardware.h"), "w", encoding="utf-8") as f:
            f.write(C_LIKE)
        if with_sck:
            sck = os.path.join(sdk, "System", "Library", "Frameworks", "ScreenCaptureKit.framework", "Headers")
            os.makedirs(sck)
            with open(os.path.join(sck, "SCStream.h"), "w", encoding="utf-8") as f:
                f.write(SCK_LIKE)
        mod = os.path.join(sdk, "System", "Library", "Frameworks", "CoreAudio.framework", "Modules")
        os.makedirs(mod)
        with open(os.path.join(mod, "ignored.h"), "w", encoding="utf-8") as f:
            f.write("extern void ShouldNotBeRead(void) API_AVAILABLE(macos(14.0));")
        return sdk

    def quiet(self, fn, *a):
        import contextlib
        import io
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = fn(*a)
        return rc, out.getvalue(), err.getvalue()

    def test_scan_and_check(self):
        sdk = self.make_sdk()
        db = os.path.join(self.dir, "db.json")
        rc, out, err = self.quiet(cma.main, ["scan-sdk", "--sdk", sdk, "--out", db, "--sdk-version", "15.2", "--min-newer", "5"])
        self.assertEqual(rc, 0, out + err)
        data = load_json(db)
        self.assertNotIn("_ShouldNotBeRead", data["symbols"], "only Headers folders are read")
        self.assertEqual(data["symbols"]["_AudioHardwareCreateProcessTap"]["v"], "14.2")

        work = os.path.join(self.dir, "work")
        os.makedirs(work)
        with open(os.path.join(work, "files.tsv"), "w", encoding="utf-8") as f:
            f.write("f1\tJukeoke.app/Contents/MacOS/jukeoke\nf2\tTools/sck_probe\n")
        for name, text in (("f1.nm", NM_OUT), ("f1.selrefs", SELREFS_OUT), ("f1.cstrings", CSTRINGS_OUT),
                           ("f2.dyld", DYLD_OUT)):
            with open(os.path.join(work, name), "w", encoding="utf-8") as f:
                f.write(text)
        rj = os.path.join(self.dir, "r.json")
        rc, out, _ = self.quiet(cma.main, ["check", "--db", db, "--work", work, "--json", rj])
        self.assertEqual(rc, 1, out)
        self.assertIn("FAIL", out)
        r = load_json(rj)
        self.assertEqual(r["fail"], 2)
        self.assertEqual(r["files"][1]["read_with"], "dyld_info -imports", "fallback when nm output is empty")

        # Only weak references and selectors: passes, with REVIEW lines.
        with open(os.path.join(work, "files.tsv"), "w", encoding="utf-8") as f:
            f.write("f2\tTools/sck_probe\n")
        rc, out, _ = self.quiet(cma.main, ["check", "--db", db, "--work", work])
        self.assertEqual(rc, 0, out)
        self.assertIn("REVIEW", out)

        # A file whose imports cannot be read is an error, never a pass.
        with open(os.path.join(work, "files.tsv"), "w", encoding="utf-8") as f:
            f.write("f3\tbroken\n")
        rc, out, _ = self.quiet(cma.main, ["check", "--db", db, "--work", work])
        self.assertEqual(rc, 2, out)

    def test_scan_refuses_when_parser_finds_too_little(self):
        sdk = self.make_sdk(with_sck=False)
        db = os.path.join(self.dir, "db.json")
        rc, _, err = self.quiet(cma.main, ["scan-sdk", "--sdk", sdk, "--out", db, "--sdk-version", "15.2", "--min-newer", "5"])
        self.assertEqual(rc, 2)
        self.assertIn("SCContentSharingPicker", err, "missing canary named")
        rc, _, err = self.quiet(cma.main, ["scan-sdk", "--sdk", sdk, "--out", db, "--sdk-version", "15.2",
                                           "--min-newer", "1000", "--canaries", ""])
        self.assertEqual(rc, 2)
        self.assertIn("not reading this SDK", err)
        rc, _, _ = self.quiet(cma.main, ["scan-sdk", "--sdk", os.path.join(self.dir, "nothing"), "--out", db])
        self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main()
