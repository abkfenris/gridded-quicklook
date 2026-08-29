//
//  BridgingHeader.h
//  PreviewExtension
//
//  Exposes gridded-ffi's C ABI to Swift. This is the simplest bridging
//  option that works cleanly with XcodeGen (`SWIFT_OBJC_BRIDGING_HEADER`)
//  without needing a hand-maintained module map or a wrapping Objective-C
//  target -- a plain bridging header is all a single Swift target needs to
//  see a C header from HEADER_SEARCH_PATHS.
//

#import "gridded_ffi.h"
