// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use thiserror::Error;

/// Unified error type for Apple code signing.
#[derive(Debug, Error)]
pub enum AppleCodesignError {
    #[error("unknown command")]
    CliUnknownCommand,

    #[error("bad argument")]
    CliBadArgument,

    #[error("{0}")]
    CliGeneralError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid Mach-O binary: {0}")]
    InvalidBinary(String),

    #[error("invalid binary index within Mach-O: {0}")]
    InvalidMachOIndex(usize),

    #[error("binary does not have code signature data")]
    BinaryNoCodeSignature,

    #[error("binary does not have code directory blob")]
    BinaryNoCodeDirectory,

    #[error("problems reported during verification")]
    VerificationProblems,

    #[error("certificate error: {0}")]
    CertificateGeneric(String),

    #[error("X.509 certificate parsing error: {0}")]
    X509Parse(String),

    #[error("unspecified cryptography error in certificate")]
    CertificateRing(ring::error::Unspecified),

    #[error("unable to locate __TEXT segment")]
    MissingText,

    #[error("unable to locate __LINKEDIT segment")]
    MissingLinkedit,

    #[error("bad header magic in {0}")]
    BadMagic(&'static str),

    #[error("data structure parse error: {0}")]
    Scroll(#[from] scroll::Error),

    #[error("malformed identifier string in code directory")]
    CodeDirectoryMalformedIdentifier,

    #[error("malformed team name string in code directory")]
    CodeDirectoryMalformedTeam,

    #[error("SuperBlob data is malformed")]
    SuperblobMalformed,

    #[error("specified path is not of a recognized type")]
    UnrecognizedPathType,

    #[error("functionality not implemented: {0}")]
    Unimplemented(&'static str),

    #[error("unknown code signature flag: {0}")]
    CodeSignatureUnknownFlag(String),

    #[error("entitlements data not valid UTF-8: {0}")]
    EntitlementsBadUtf8(std::str::Utf8Error),

    #[error("unknown executable segment flag: {0}")]
    ExecutableSegmentUnknownFlag(String),

    #[error("__LINKEDIT isn't final Mach-O segment")]
    LinkeditNotLast,

    #[error("__LINKEDIT segment contains data after signature")]
    DataAfterSignature,

    #[error("insufficient room to write code signature load command")]
    LoadCommandNoRoom,

    #[error("error writing Mach-O: {0}")]
    MachOWrite(String),

    #[error("no identifier string provided")]
    NoIdentifier,

    #[error("no signing certificate")]
    NoSigningCertificate,

    #[error("signature data too large (please report this issue)")]
    SignatureDataTooLarge,

    #[error("invalid builder operation: {0}")]
    SignatureBuilder(&'static str),

    #[error("unknown digest algorithm")]
    DigestUnknownAlgorithm,

    #[error("unsupported digest algorithm")]
    DigestUnsupportedAlgorithm,

    #[error("unspecified digest error")]
    DigestUnspecified,

    #[error("nested bundle does not exist: {0}")]
    BundleUnknown(String),

    #[error(
        "unexpected resource rule evaluation when signing nested bundle (please report this issue)"
    )]
    BundleUnexpectedResourceRuleResult,

    #[error("unable to parse settings scope: {0}")]
    ParseSettingsScope(String),

    #[error("incorrect password given when decrypting PFX data")]
    PfxBadPassword,

    #[error("error parsing PFX data: {0}")]
    PfxParseError(String),

    #[error("error interfacing with macOS keychain: {0}")]
    KeychainError(String),

    #[error("error interfacing with Windows certificate store: {0}")]
    WindowsStoreError(String),

    #[error("failed to find certificate satisfying requirements: {0}")]
    CertificateNotFound(String),

    #[error("the given OID does not match a recognized Apple certificate authority extension")]
    OidIsntCertificateAuthority,

    #[error("the given OID does not match a recognized Apple extended key usage extension")]
    OidIsntExtendedKeyUsage,

    #[error("the given OID does not match a recognized Apple code signing extension")]
    OidIsntCodeSigningExtension,

    #[error("error building certificate: {0}")]
    CertificateBuildError(String),

    #[error("unknown certificate profile: {0}")]
    UnknownCertificateProfile(String),

    #[error("unknown code execution policy: {0}")]
    UnknownPolicy(String),

    #[error("unable to generate code requirement policy: {0}")]
    PolicyFormulationError(String),

    #[error("signing XAR archives requires a signing certificate")]
    XarNoAdhoc,

    #[error("App Store Connect API Key error: {0}")]
    AppStoreConnectApiKey(String),

    #[error("Could not find App Store Connect API key in default search locations")]
    AppStoreConnectApiKeyNotFound,

    #[error("signing settings are not compatible with notarization")]
    ForNotarizationInvalidSettings,

    #[error("no authentication credentials to perform notarization request")]
    NotarizeNoAuthCredentials,

    #[error("reached time limit waiting for notarization to complete")]
    NotarizeWaitLimitReached,

    #[error("error interacting with Notary API")]
    NotarizeServerError,

    #[error("notarization rejected: StatusCode={0}; StatusMessage={1}")]
    NotarizeRejected(i64, String),

    #[error("notarization is incomplete (no status code and message)")]
    NotarizeIncomplete,

    #[error("notarization package is invalid")]
    NotarizeInvalid,

    #[error("notarization record not in response: {0}")]
    NotarizationRecordNotInResponse(String),

    #[error("signed ticket data not found in ticket lookup response (this should not happen)")]
    NotarizationRecordNoSignedTicket,

    #[error("signedTicket in notarization ticket lookup response is not BYTES: {0}")]
    NotarizationRecordSignedTicketNotBytes(String),

    #[error("notarization ticket lookup failure: {0}: {1}")]
    NotarizationLookupFailure(String, String),

    #[error("unable to determine app platform from bundle")]
    BundleUnknownAppPlatform,

    #[error("XAR file is malformed; cannot staple")]
    StapleMalformedXar,

    #[error("failed to find main executable in bundle")]
    StapleMainExecutableNotFound,

    #[error("bad header magic in DMG; not a DMG file?")]
    DmgBadMagic,

    #[error("cannot notarize DMG without an embedded signature")]
    DmgNotarizeNoSignature,

    #[error("cannot staple DMG without an embedded signature")]
    DmgStapleNoSignature,

    #[error("failed to find certificate in smartcard slot {0}")]
    SmartcardNoCertificate(String),

    #[error("failed to authenticate with smartcard device")]
    SmartcardFailedAuthentication,

    #[error("poisoned lock")]
    PoisonedLock,

    #[error("internal API / logic error: {0}")]
    LogicError(String),
}

/// Result type for this library.
pub type Result<T, E = AppleCodesignError> = std::result::Result<T, E>;
