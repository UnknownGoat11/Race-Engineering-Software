// LMU shared-memory ABI layout probe.
//
// Compiles against the *installed* Studio 397 / LMU SDK headers (passed via an include
// path pointing at <LMU>\Support\SharedMemoryInterface -- the headers are never copied
// into this repository, their license forbids redistribution) and prints, as JSON:
//   * sizeof / alignof of every struct reachable from SharedMemoryObjectOut
//   * offsetof / sizeof / canonical type of every field (hand-listed, then checked for
//     completeness by byte-coverage + alignment analysis; see analyze())
//   * SME_* / IP_VehicleClass / IP_VehicleChampionship enum values, MAX_PATH
//   * the mapping / event names
//   * absolute offsets of key paths inside SharedMemoryLayout
//
// Usage:  probe.exe [--out <file>] [--emit-asserts]
//   --emit-asserts  instead of JSON, emit a C++ file of static_asserts that pins every
//                   recorded size/offset; compiling it with another compiler against the
//                   same headers proves that compiler lays the structs out identically.
// Exit code: 0 = all self-checks passed, 2 = at least one check failed (output is still
// written; look at "checks.errors"), 1 = usage / IO error.

#ifndef _CRT_SECURE_NO_WARNINGS
#define _CRT_SECURE_NO_WARNINGS  // plain fopen() is fine here
#endif
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <type_traits>
#include <typeinfo>
#include <utility>
#include <vector>

#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

// The SDK headers trip a few warnings at /W4 / -Wextra that are not ours to fix
// (unused parameters in empty virtual stubs, nameless struct in a union, an unused
// static helper). Silence them for the SDK include only.
#if defined(_MSC_VER) && !defined(__clang__)
#pragma warning(push)
#pragma warning(disable : 4100 4201 4505)
#elif defined(__clang__)
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunused-parameter"
#pragma clang diagnostic ignored "-Wunused-function"
#pragma clang diagnostic ignored "-Wpragma-pack"
#pragma clang diagnostic ignored "-Wnested-anon-types"
#pragma clang diagnostic ignored "-Wgnu-anonymous-struct"
#pragma clang diagnostic ignored "-Woverloaded-virtual"
#elif defined(__GNUC__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
#pragma GCC diagnostic ignored "-Wunused-function"
#endif
#include "SharedMemoryInterface.hpp"
#if defined(_MSC_VER) && !defined(__clang__)
#pragma warning(pop)
#elif defined(__clang__)
#pragma clang diagnostic pop
#elif defined(__GNUC__)
#pragma GCC diagnostic pop
#endif

namespace {

// Declared outside every #pragma pack region: measures the compiler's default packing for
// 8-byte members. SharedMemoryInterface.hpp has no #pragma pack of its own, so its structs
// (SharedMemory*) are laid out with this default; InternalsPlugin.hpp wraps its structs
// in #pragma pack(push, 4).
struct DefaultPackProbe {
    char c;
    double d;
};
constexpr size_t kInternalsPluginPack = 4;
constexpr size_t kDefaultPack = offsetof(DefaultPackProbe, d);

// ----------------------------------------------------------------------------------------
// Private nested type SharedMemoryLock::LockData (the layout of mapping
// "LMU_SharedMemoryLockData"). Reached without copying its definition: explicit template
// instantiation is exempt from access checking ([temp.explicit]), so we can name the
// private member pointer &SharedMemoryLock::mDataPtr there and recover its type.
struct LockDataPtrTag {};
template <typename Tag, auto M>
struct PrivateMemberPtr {
    friend constexpr auto get_private(Tag) { return M; }
};
constexpr auto get_private(LockDataPtrTag);
template struct PrivateMemberPtr<LockDataPtrTag, &SharedMemoryLock::mDataPtr>;
template <typename C, typename M>
M member_type_of(M C::*);
using LockDataT = std::remove_pointer_t<decltype(member_type_of(get_private(LockDataPtrTag{})))>;

// ----------------------------------------------------------------------------------------
// Canonical, compiler-independent type strings (typeid names differ between MSVC and
// Itanium-ABI compilers, which would make JSON diffs across compilers noisy).
template <typename T>
struct NamedType {
    static std::string name() { return std::string("?typeid:") + typeid(T).name(); }
};
#define LP_NAMED(T) \
    template <>     \
    struct NamedType<T> { \
        static std::string name() { return #T; } \
    }
LP_NAMED(TelemVect3);
LP_NAMED(TelemWheelV01);
LP_NAMED(TelemInfoV01);
LP_NAMED(VehicleScoringInfoV01);
LP_NAMED(ScoringInfoV01);
LP_NAMED(ApplicationStateV01);
LP_NAMED(SharedMemoryEvent);
LP_NAMED(SharedMemoryGeneric);
LP_NAMED(SharedMemoryPathData);
LP_NAMED(SharedMemoryScoringData);
LP_NAMED(SharedMemoryTelemetryData);
LP_NAMED(SharedMemoryObjectOut);
LP_NAMED(SharedMemoryLayout);
LP_NAMED(IP_VehicleClass);
LP_NAMED(IP_VehicleChampionship);
LP_NAMED(HWND__);
#undef LP_NAMED

template <typename T>
std::string int_kind() {
    return std::string(std::is_signed_v<T> ? "i" : "u") + std::to_string(sizeof(T) * 8);
}

template <typename T>
std::string ctype();

template <typename T>
std::string ctype_nonarray() {
    if constexpr (std::is_same_v<T, bool>) return "bool";
    else if constexpr (std::is_same_v<T, char>) return "char";
    else if constexpr (std::is_same_v<T, signed char>) return "signed char";
    else if constexpr (std::is_same_v<T, unsigned char>) return "unsigned char";
    else if constexpr (std::is_same_v<T, short>) return "short";
    else if constexpr (std::is_same_v<T, unsigned short>) return "unsigned short";
    else if constexpr (std::is_same_v<T, int>) return "int";
    else if constexpr (std::is_same_v<T, unsigned int>) return "unsigned int";
    else if constexpr (std::is_same_v<T, long>) return "long";
    else if constexpr (std::is_same_v<T, unsigned long>) return "unsigned long";
    else if constexpr (std::is_same_v<T, long long>) return "long long";
    else if constexpr (std::is_same_v<T, unsigned long long>) return "unsigned long long";
    else if constexpr (std::is_same_v<T, float>) return "float";
    else if constexpr (std::is_same_v<T, double>) return "double";
    else if constexpr (std::is_pointer_v<T>) return ctype<std::remove_pointer_t<T>>() + "*";
    else if constexpr (std::is_enum_v<T>) return "enum " + NamedType<T>::name();
    else if constexpr (std::is_union_v<T>) return "union " + NamedType<T>::name();
    else if constexpr (std::is_class_v<T>) return "struct " + NamedType<T>::name();
    else return std::string("?typeid:") + typeid(T).name();
}

template <typename T>
void dims_of(std::vector<size_t>& dims) {
    if constexpr (std::is_array_v<T>) {
        dims.push_back(std::extent_v<T>);
        dims_of<std::remove_extent_t<T>>(dims);
    }
}

template <typename T>
std::string ctype() {
    using U = std::remove_cv_t<T>;
    std::string q;
    if constexpr (std::is_volatile_v<T>) q += "volatile ";
    if constexpr (std::is_const_v<T>) q += "const ";
    if constexpr (std::is_array_v<U>) {
        std::vector<size_t> dims;
        dims_of<U>(dims);
        std::string s = ctype<std::remove_all_extents_t<U>>();
        for (size_t d : dims) s += "[" + std::to_string(d) + "]";
        return s;
    } else {
        return q + ctype_nonarray<U>();
    }
}

// Coarse kind of the element type, meant for code generators (e.g. the Rust reader).
template <typename E>
std::string kind_of() {
    if constexpr (std::is_same_v<E, bool>) return "bool";
    else if constexpr (std::is_same_v<E, char>) return "char";
    else if constexpr (std::is_enum_v<E>) return "enum:" + int_kind<std::underlying_type_t<E>>();
    else if constexpr (std::is_floating_point_v<E>) return sizeof(E) == 4 ? "f32" : "f64";
    else if constexpr (std::is_integral_v<E>) return int_kind<E>();
    else if constexpr (std::is_pointer_v<E>) return "ptr";
    else if constexpr (std::is_union_v<E>) return "union";
    else if constexpr (std::is_class_v<E>) return "struct";
    else return "unknown";
}

// ----------------------------------------------------------------------------------------
struct FieldRec {
    std::string name;
    size_t offset = 0;
    size_t size = 0;        // sizeof(decltype(S::m))
    size_t macro_size = 0;  // sizeof(S::m) as written at the call site (must equal size)
    size_t align = 0;       // alignof(type) (natural; placement is capped by the struct's pack)
    size_t elem_size = 0;
    std::string type;
    std::string kind;
    std::vector<size_t> dims;
    int union_group = -1;   // >= 0: member of an anonymous union
    int union_alt = -1;     // which alternative of that union
};

struct HoleRec {
    size_t offset = 0;
    size_t size = 0;
    std::string after;
    std::string before;
    bool tail = false;
    bool explained = false;
};

struct StructRec {
    std::string name;
    std::string header;
    size_t size = 0;
    size_t align = 0;
    size_t pack = 0;
    std::string pack_source;
    std::vector<FieldRec> fields;
    // filled by analyze()
    std::vector<HoleRec> holes;
    size_t covered_bytes = 0;
    size_t sum_field_sizes = 0;  // excluding non-first union alternatives
    size_t fields_below_natural_alignment = 0;
    std::vector<std::string> errors;
};

template <typename T>
FieldRec make_field(const char* name, size_t offset, size_t macro_size, int ugroup, int ualt) {
    using E = std::remove_cv_t<std::remove_all_extents_t<T>>;
    FieldRec f;
    f.name = name;
    f.offset = offset;
    f.size = sizeof(T);
    f.macro_size = macro_size;
    f.align = alignof(T);
    f.elem_size = sizeof(E);
    f.type = ctype<T>();
    f.kind = kind_of<E>();
    dims_of<std::remove_cv_t<T>>(f.dims);
    f.union_group = ugroup;
    f.union_alt = ualt;
    return f;
}

template <typename S>
StructRec begin_struct(const char* name, const char* header, size_t pack, const char* pack_source) {
    StructRec s;
    s.name = name;
    s.header = header;
    s.size = sizeof(S);
    s.align = alignof(S);
    s.pack = pack;
    s.pack_source = pack_source;
    return s;
}

// Per-field offsetof / sizeof, hand-listed in declaration order. `S` and `s` must be in scope.
#define LP_FIELD(m) s.fields.push_back(make_field<decltype(S::m)>(#m, offsetof(S, m), sizeof(S::m), -1, -1))
#define LP_UFIELD(m, g, a) s.fields.push_back(make_field<decltype(S::m)>(#m, offsetof(S, m), sizeof(S::m), g, a))

constexpr const char* kIP = "InternalsPlugin.hpp";
constexpr const char* kSMI = "SharedMemoryInterface.hpp";
constexpr const char* kPack4 = "#pragma pack(push, 4) in InternalsPlugin.hpp";
constexpr const char* kPackDefault = "compiler default (SharedMemoryInterface.hpp has no #pragma pack); measured via DefaultPackProbe";

StructRec probe_TelemVect3() {
    using S = TelemVect3;
    StructRec s = begin_struct<S>("TelemVect3", kIP, kInternalsPluginPack, kPack4);
    // anonymous union { struct { double x, y, z; }; double data[3]; }
    LP_UFIELD(x, 0, 0);
    LP_UFIELD(y, 0, 0);
    LP_UFIELD(z, 0, 0);
    LP_UFIELD(data, 0, 1);
    return s;
}

StructRec probe_TelemWheelV01() {
    using S = TelemWheelV01;
    StructRec s = begin_struct<S>("TelemWheelV01", kIP, kInternalsPluginPack, kPack4);
    LP_FIELD(mSuspensionDeflection);
    LP_FIELD(mRideHeight);
    LP_FIELD(mSuspForce);
    LP_FIELD(mBrakeTemp);
    LP_FIELD(mBrakePressure);
    LP_FIELD(mRotation);
    LP_FIELD(mLateralPatchVel);
    LP_FIELD(mLongitudinalPatchVel);
    LP_FIELD(mLateralGroundVel);
    LP_FIELD(mLongitudinalGroundVel);
    LP_FIELD(mCamber);
    LP_FIELD(mLateralForce);
    LP_FIELD(mLongitudinalForce);
    LP_FIELD(mTireLoad);
    LP_FIELD(mGripFract);
    LP_FIELD(mPressure);
    LP_FIELD(mTemperature);
    LP_FIELD(mWear);
    LP_FIELD(mTerrainName);
    LP_FIELD(mSurfaceType);
    LP_FIELD(mFlat);
    LP_FIELD(mDetached);
    LP_FIELD(mStaticUndeflectedRadius);
    LP_FIELD(mVerticalTireDeflection);
    LP_FIELD(mWheelYLocation);
    LP_FIELD(mToe);
    LP_FIELD(mTireCarcassTemperature);
    LP_FIELD(mTireInnerLayerTemperature);
    LP_FIELD(mOptimalTemp);
    LP_FIELD(mCompoundIndex);
    LP_FIELD(mCompoundType);
    LP_FIELD(mExpansion);
    return s;
}

StructRec probe_TelemInfoV01() {
    using S = TelemInfoV01;
    StructRec s = begin_struct<S>("TelemInfoV01", kIP, kInternalsPluginPack, kPack4);
    LP_FIELD(mID);
    LP_FIELD(mDeltaTime);
    LP_FIELD(mElapsedTime);
    LP_FIELD(mLapNumber);
    LP_FIELD(mLapStartET);
    LP_FIELD(mVehicleName);
    LP_FIELD(mTrackName);
    LP_FIELD(mPos);
    LP_FIELD(mLocalVel);
    LP_FIELD(mLocalAccel);
    LP_FIELD(mOri);
    LP_FIELD(mLocalRot);
    LP_FIELD(mLocalRotAccel);
    LP_FIELD(mGear);
    LP_FIELD(mEngineRPM);
    LP_FIELD(mEngineWaterTemp);
    LP_FIELD(mEngineOilTemp);
    LP_FIELD(mClutchRPM);
    LP_FIELD(mUnfilteredThrottle);
    LP_FIELD(mUnfilteredBrake);
    LP_FIELD(mUnfilteredSteering);
    LP_FIELD(mUnfilteredClutch);
    LP_FIELD(mFilteredThrottle);
    LP_FIELD(mFilteredBrake);
    LP_FIELD(mFilteredSteering);
    LP_FIELD(mFilteredClutch);
    LP_FIELD(mSteeringShaftTorque);
    LP_FIELD(mFront3rdDeflection);
    LP_FIELD(mRear3rdDeflection);
    LP_FIELD(mFrontWingHeight);
    LP_FIELD(mFrontRideHeight);
    LP_FIELD(mRearRideHeight);
    LP_FIELD(mDrag);
    LP_FIELD(mFrontDownforce);
    LP_FIELD(mRearDownforce);
    LP_FIELD(mFuel);
    LP_FIELD(mEngineMaxRPM);
    LP_FIELD(mScheduledStops);
    LP_FIELD(mOverheating);
    LP_FIELD(mDetached);
    LP_FIELD(mHeadlights);
    LP_FIELD(mDentSeverity);
    LP_FIELD(mLastImpactET);
    LP_FIELD(mLastImpactMagnitude);
    LP_FIELD(mLastImpactPos);
    LP_FIELD(mEngineTorque);
    LP_FIELD(mCurrentSector);
    LP_FIELD(mSpeedLimiter);
    LP_FIELD(mMaxGears);
    LP_FIELD(mFrontTireCompoundIndex);
    LP_FIELD(mRearTireCompoundIndex);
    LP_FIELD(mFuelCapacity);
    LP_FIELD(mFrontFlapActivated);
    LP_FIELD(mRearFlapActivated);
    LP_FIELD(mRearFlapLegalStatus);
    LP_FIELD(mIgnitionStarter);
    LP_FIELD(mFrontTireCompoundName);
    LP_FIELD(mRearTireCompoundName);
    LP_FIELD(mSpeedLimiterAvailable);
    LP_FIELD(mAntiStallActivated);
    LP_FIELD(mUnused);
    LP_FIELD(mVisualSteeringWheelRange);
    LP_FIELD(mRearBrakeBias);
    LP_FIELD(mTurboBoostPressure);
    LP_FIELD(mPhysicsToGraphicsOffset);
    LP_FIELD(mPhysicalSteeringWheelRange);
    LP_FIELD(mDeltaBest);
    LP_FIELD(mBatteryChargeFraction);
    LP_FIELD(mElectricBoostMotorTorque);
    LP_FIELD(mElectricBoostMotorRPM);
    LP_FIELD(mElectricBoostMotorTemperature);
    LP_FIELD(mElectricBoostWaterTemperature);
    LP_FIELD(mElectricBoostMotorState);
    LP_FIELD(mLapInvalidated);
    LP_FIELD(mABSActive);
    LP_FIELD(mTCActive);
    LP_FIELD(mSpeedLimiterActive);
    LP_FIELD(mWiperState);
    LP_FIELD(mTC);
    LP_FIELD(mTCMax);
    LP_FIELD(mTCSlip);
    LP_FIELD(mTCSlipMax);
    LP_FIELD(mTCCut);
    LP_FIELD(mTCCutMax);
    LP_FIELD(mABS);
    LP_FIELD(mABSMax);
    LP_FIELD(mMotorMap);
    LP_FIELD(mMotorMapMax);
    LP_FIELD(mMigration);
    LP_FIELD(mMigrationMax);
    LP_FIELD(mFrontAntiSway);
    LP_FIELD(mFrontAntiSwayMax);
    LP_FIELD(mRearAntiSway);
    LP_FIELD(mRearAntiSwayMax);
    LP_FIELD(mLiftAndCoastProgress);
    LP_FIELD(mTrackLimitsSteps);
    LP_FIELD(mRegen);
    LP_FIELD(mSoC);
    LP_FIELD(mVirtualEnergy);
    LP_FIELD(mTimeGapCarAhead);
    LP_FIELD(mTimeGapCarBehind);
    LP_FIELD(mTimeGapPlaceAhead);
    LP_FIELD(mTimeGapPlaceBehind);
    LP_FIELD(mVehicleModel);
    LP_FIELD(mVehicleClass);
    LP_FIELD(mVehicleChampionship);
    LP_FIELD(mExpansion);
    LP_FIELD(mWheel);
    return s;
}

StructRec probe_VehicleScoringInfoV01() {
    using S = VehicleScoringInfoV01;
    StructRec s = begin_struct<S>("VehicleScoringInfoV01", kIP, kInternalsPluginPack, kPack4);
    LP_FIELD(mID);
    LP_FIELD(mDriverName);
    LP_FIELD(mVehicleName);
    LP_FIELD(mTotalLaps);
    LP_FIELD(mSector);
    LP_FIELD(mFinishStatus);
    LP_FIELD(mLapDist);
    LP_FIELD(mPathLateral);
    LP_FIELD(mTrackEdge);
    LP_FIELD(mBestSector1);
    LP_FIELD(mBestSector2);
    LP_FIELD(mBestLapTime);
    LP_FIELD(mLastSector1);
    LP_FIELD(mLastSector2);
    LP_FIELD(mLastLapTime);
    LP_FIELD(mCurSector1);
    LP_FIELD(mCurSector2);
    LP_FIELD(mNumPitstops);
    LP_FIELD(mNumPenalties);
    LP_FIELD(mIsPlayer);
    LP_FIELD(mControl);
    LP_FIELD(mInPits);
    LP_FIELD(mPlace);
    LP_FIELD(mVehicleClass);
    LP_FIELD(mTimeBehindNext);
    LP_FIELD(mLapsBehindNext);
    LP_FIELD(mTimeBehindLeader);
    LP_FIELD(mLapsBehindLeader);
    LP_FIELD(mLapStartET);
    LP_FIELD(mPos);
    LP_FIELD(mLocalVel);
    LP_FIELD(mLocalAccel);
    LP_FIELD(mOri);
    LP_FIELD(mLocalRot);
    LP_FIELD(mLocalRotAccel);
    LP_FIELD(mHeadlights);
    LP_FIELD(mPitState);
    LP_FIELD(mServerScored);
    LP_FIELD(mIndividualPhase);
    LP_FIELD(mQualification);
    LP_FIELD(mTimeIntoLap);
    LP_FIELD(mEstimatedLapTime);
    LP_FIELD(mPitGroup);
    LP_FIELD(mFlag);
    LP_FIELD(mUnderYellow);
    LP_FIELD(mCountLapFlag);
    LP_FIELD(mInGarageStall);
    LP_FIELD(mUpgradePack);
    LP_FIELD(mPitLapDist);
    LP_FIELD(mBestLapSector1);
    LP_FIELD(mBestLapSector2);
    LP_FIELD(mSteamID);
    LP_FIELD(mVehFilename);
    LP_FIELD(mAttackMode);
    LP_FIELD(mFuelFraction);
    LP_FIELD(mDRSState);
    LP_FIELD(mExpansion);
    return s;
}

StructRec probe_ScoringInfoV01() {
    using S = ScoringInfoV01;
    StructRec s = begin_struct<S>("ScoringInfoV01", kIP, kInternalsPluginPack, kPack4);
    LP_FIELD(mTrackName);
    LP_FIELD(mSession);
    LP_FIELD(mCurrentET);
    LP_FIELD(mEndET);
    LP_FIELD(mMaxLaps);
    LP_FIELD(mLapDist);
    LP_FIELD(mResultsStream);
    LP_FIELD(mNumVehicles);
    LP_FIELD(mGamePhase);
    LP_FIELD(mYellowFlagState);
    LP_FIELD(mSectorFlag);
    LP_FIELD(mStartLight);
    LP_FIELD(mNumRedLights);
    LP_FIELD(mInRealtime);
    LP_FIELD(mPlayerName);
    LP_FIELD(mPlrFileName);
    LP_FIELD(mDarkCloud);
    LP_FIELD(mRaining);
    LP_FIELD(mAmbientTemp);
    LP_FIELD(mTrackTemp);
    LP_FIELD(mWind);
    LP_FIELD(mMinPathWetness);
    LP_FIELD(mMaxPathWetness);
    LP_FIELD(mGameMode);
    LP_FIELD(mIsPasswordProtected);
    LP_FIELD(mServerPort);
    LP_FIELD(mServerPublicIP);
    LP_FIELD(mMaxPlayers);
    LP_FIELD(mServerName);
    LP_FIELD(mStartET);
    LP_FIELD(mAvgPathWetness);
    LP_FIELD(mSessionTimeRemaining);
    LP_FIELD(mTimeOfDay);
    LP_FIELD(mIsFixedSetup);
    LP_FIELD(mTrackGripLevel);
    LP_FIELD(mCloudCoverage);
    LP_FIELD(mTrackLimitsStepsPerPenalty);
    LP_FIELD(mTrackLimitsStepsPerPoint);
    LP_FIELD(mExpansion);
    LP_FIELD(mVehicle);
    return s;
}

StructRec probe_ApplicationStateV01() {
    using S = ApplicationStateV01;
    StructRec s = begin_struct<S>("ApplicationStateV01", kIP, kInternalsPluginPack, kPack4);
    LP_FIELD(mAppWindow);
    LP_FIELD(mWidth);
    LP_FIELD(mHeight);
    LP_FIELD(mRefreshRate);
    LP_FIELD(mWindowed);
    LP_FIELD(mOptionsLocation);
    LP_FIELD(mOptionsPage);
    LP_FIELD(mExpansion);
    return s;
}

StructRec probe_SharedMemoryGeneric() {
    using S = SharedMemoryGeneric;
    StructRec s = begin_struct<S>("SharedMemoryGeneric", kSMI, kDefaultPack, kPackDefault);
    LP_FIELD(events);
    LP_FIELD(gameVersion);
    LP_FIELD(FFBTorque);
    LP_FIELD(appInfo);
    return s;
}

StructRec probe_SharedMemoryPathData() {
    using S = SharedMemoryPathData;
    StructRec s = begin_struct<S>("SharedMemoryPathData", kSMI, kDefaultPack, kPackDefault);
    LP_FIELD(userData);
    LP_FIELD(customVariables);
    LP_FIELD(stewardResults);
    LP_FIELD(playerProfile);
    LP_FIELD(pluginsFolder);
    return s;
}

StructRec probe_SharedMemoryScoringData() {
    using S = SharedMemoryScoringData;
    StructRec s = begin_struct<S>("SharedMemoryScoringData", kSMI, kDefaultPack, kPackDefault);
    LP_FIELD(scoringInfo);
    LP_FIELD(scoringStreamSize);
    LP_FIELD(vehScoringInfo);
    LP_FIELD(scoringStream);
    return s;
}

StructRec probe_SharedMemoryTelemetryData() {
    using S = SharedMemoryTelemetryData;
    StructRec s = begin_struct<S>("SharedMemoryTelemetryData", kSMI, kDefaultPack, kPackDefault);
    LP_FIELD(activeVehicles);
    LP_FIELD(playerVehicleIdx);
    LP_FIELD(playerHasVehicle);
    LP_FIELD(telemInfo);
    return s;
}

StructRec probe_SharedMemoryObjectOut() {
    using S = SharedMemoryObjectOut;
    StructRec s = begin_struct<S>("SharedMemoryObjectOut", kSMI, kDefaultPack, kPackDefault);
    LP_FIELD(generic);
    LP_FIELD(paths);
    LP_FIELD(scoring);
    LP_FIELD(telemetry);
    return s;
}

StructRec probe_SharedMemoryLayout() {
    using S = SharedMemoryLayout;
    StructRec s = begin_struct<S>("SharedMemoryLayout", kSMI, kDefaultPack, kPackDefault);
    LP_FIELD(data);
    return s;
}

StructRec probe_LockData() {
    using S = LockDataT;
    StructRec s = begin_struct<S>("SharedMemoryLock::LockData", kSMI, kDefaultPack, kPackDefault);
    LP_FIELD(waiters);
    LP_FIELD(busy);
    return s;
}

#undef LP_FIELD
#undef LP_UFIELD

size_t round_up(size_t v, size_t a) { return a == 0 ? v : (v + a - 1) / a * a; }

// Completeness / sanity analysis of one hand-listed struct:
//  * every field lies inside the struct, fields only overlap as alternatives of one union
//  * declaration order == offset order
//  * each uncovered byte run ("hole") must be explained by alignment:
//      interior hole [a,b): b == round_up(a, min(alignof(next field), pack))
//      tail hole    [a,size): size == round_up(a, alignof(struct))
//    A forgotten field would show up as an unexplained hole (or a too-large one).
void analyze(StructRec& s) {
    std::vector<unsigned char> cov(s.size, 0);
    for (const FieldRec& f : s.fields) {
        if (f.size != f.macro_size)
            s.errors.push_back(f.name + ": sizeof(decltype) != sizeof(S::m)");
        if (f.offset + f.size > s.size) {
            s.errors.push_back(f.name + ": extends past end of struct");
            continue;
        }
        for (size_t i = f.offset; i < f.offset + f.size; ++i) cov[i] = 1;
        const size_t eff = f.align < s.pack ? f.align : s.pack;
        if (f.offset % eff != 0)
            s.errors.push_back(f.name + ": offset not a multiple of min(alignof, pack) -- pack assumption wrong?");
        if (f.offset % f.align != 0) ++s.fields_below_natural_alignment;
        if (f.union_alt <= 0) s.sum_field_sizes += f.size;
    }
    for (size_t i = 0; i < s.fields.size(); ++i) {
        for (size_t j = i + 1; j < s.fields.size(); ++j) {
            const FieldRec& a = s.fields[i];
            const FieldRec& b = s.fields[j];
            const bool overlap = a.offset < b.offset + b.size && b.offset < a.offset + a.size;
            const bool same_union_other_alt =
                a.union_group >= 0 && a.union_group == b.union_group && a.union_alt != b.union_alt;
            if (overlap && !same_union_other_alt)
                s.errors.push_back(a.name + " overlaps " + b.name);
            if (!same_union_other_alt && b.offset < a.offset)
                s.errors.push_back(b.name + " declared after " + a.name + " but has a lower offset");
        }
    }
    size_t i = 0;
    while (i < s.size) {
        if (cov[i]) {
            ++s.covered_bytes;
            ++i;
            continue;
        }
        size_t j = i;
        while (j < s.size && !cov[j]) ++j;
        HoleRec h;
        h.offset = i;
        h.size = j - i;
        h.tail = (j == s.size);
        for (const FieldRec& f : s.fields)
            if (f.offset + f.size == i) h.after = f.name;
        if (h.tail) {
            h.explained = i > 0 && round_up(i, s.align) == s.size;
        } else {
            const FieldRec* next = nullptr;
            for (const FieldRec& f : s.fields)
                if (f.offset == j) { next = &f; break; }
            if (next) {
                h.before = next->name;
                const size_t eff = next->align < s.pack ? next->align : s.pack;
                h.explained = i > 0 && round_up(i, eff) == j;
            }
        }
        if (!h.explained)
            s.errors.push_back("unexplained hole at " + std::to_string(h.offset) + " (" +
                               std::to_string(h.size) + " bytes) -- missing field?");
        s.holes.push_back(h);
        i = j;
    }
}

// ----------------------------------------------------------------------------------------
class JsonWriter {
public:
    std::string out;
    void begin_object(const std::string& key = "", bool inl = false) { open(key, '{', inl); }
    void end_object() { close('}'); }
    void begin_array(const std::string& key = "", bool inl = false) { open(key, '[', inl); }
    void end_array() { close(']'); }
    void str(const std::string& key, const std::string& v) { prefix(key); quote(v); }
    void num(const std::string& key, unsigned long long v) { prefix(key); out += std::to_string(v); }
    void inum(const std::string& key, long long v) { prefix(key); out += std::to_string(v); }
    void boolean(const std::string& key, bool v) { prefix(key); out += v ? "true" : "false"; }

private:
    struct Level {
        int count;
        bool inl;
    };
    std::vector<Level> stack_;
    void newline() {
        out += '\n';
        out.append(stack_.size() * 2, ' ');
    }
    void prefix(const std::string& key) {
        if (!stack_.empty()) {
            Level& l = stack_.back();
            if (l.count++ > 0) out += l.inl ? ", " : ",";
            if (!l.inl) newline();
        }
        if (!key.empty()) {
            quote(key);
            out += ": ";
        }
    }
    void open(const std::string& key, char c, bool inl) {
        const bool parent_inline = !stack_.empty() && stack_.back().inl;
        prefix(key);
        out += c;
        stack_.push_back({0, inl || parent_inline});
    }
    void close(char c) {
        const Level l = stack_.back();
        stack_.pop_back();
        if (!l.inl && l.count > 0) newline();
        out += c;
    }
    void quote(const std::string& v) {
        out += '"';
        for (unsigned char ch : v) {
            switch (ch) {
                case '"': out += "\\\""; break;
                case '\\': out += "\\\\"; break;
                case '\n': out += "\\n"; break;
                case '\r': out += "\\r"; break;
                case '\t': out += "\\t"; break;
                default:
                    if (ch < 0x20 || ch >= 0x7f) {
                        char buf[8];
                        std::snprintf(buf, sizeof(buf), "\\u%04x", static_cast<unsigned>(ch));
                        out += buf;
                    } else {
                        out += static_cast<char>(ch);
                    }
            }
        }
        out += '"';
    }
};

struct EnumVal {
    const char* name;
    const char* expr;  // expression usable in generated static_asserts
    long long value;
};

#define LP_EV(e) EnumVal{#e, #e, static_cast<long long>(e)}
#define LP_EVC(E, e) EnumVal{#e, #E "::" #e, static_cast<long long>(E::e)}

const std::vector<EnumVal>& sme_values() {
    static const std::vector<EnumVal> v = {
        LP_EV(SME_ENTER),           LP_EV(SME_EXIT),         LP_EV(SME_STARTUP),
        LP_EV(SME_SHUTDOWN),        LP_EV(SME_LOAD),         LP_EV(SME_UNLOAD),
        LP_EV(SME_START_SESSION),   LP_EV(SME_END_SESSION),  LP_EV(SME_ENTER_REALTIME),
        LP_EV(SME_EXIT_REALTIME),   LP_EV(SME_UPDATE_SCORING), LP_EV(SME_UPDATE_TELEMETRY),
        LP_EV(SME_INIT_APPLICATION), LP_EV(SME_UNINIT_APPLICATION), LP_EV(SME_SET_ENVIRONMENT),
        LP_EV(SME_FFB),             LP_EV(SME_MAX),
    };
    return v;
}

const std::vector<EnumVal>& vehicle_class_values() {
    static const std::vector<EnumVal> v = {
        LP_EVC(IP_VehicleClass, Hypercar), LP_EVC(IP_VehicleClass, LMP2_ELMS),
        LP_EVC(IP_VehicleClass, LMP2),     LP_EVC(IP_VehicleClass, LMP3),
        LP_EVC(IP_VehicleClass, GTE),      LP_EVC(IP_VehicleClass, GT3),
        LP_EVC(IP_VehicleClass, PaceCar),  LP_EVC(IP_VehicleClass, Unknown),
    };
    return v;
}

const std::vector<EnumVal>& vehicle_championship_values() {
    static const std::vector<EnumVal> v = {
        LP_EVC(IP_VehicleChampionship, WEC_2023),  LP_EVC(IP_VehicleChampionship, WEC_2024),
        LP_EVC(IP_VehicleChampionship, WEC_2025),  LP_EVC(IP_VehicleChampionship, WEC_2026),
        LP_EVC(IP_VehicleChampionship, ELMS_2025), LP_EVC(IP_VehicleChampionship, ELMS_2026),
        LP_EVC(IP_VehicleChampionship, Unknown),
    };
    return v;
}
#undef LP_EV
#undef LP_EVC

template <typename E>
void emit_enum(JsonWriter& w, const char* name, const char* header, const std::vector<EnumVal>& vals) {
    w.begin_object(name);
    w.str("header", header);
    w.num("sizeof", sizeof(E));
    w.str("underlying", ctype<std::underlying_type_t<E>>());
    w.boolean("scoped", !std::is_convertible_v<E, long long>);
    w.begin_object("values");
    for (const EnumVal& v : vals) w.inum(v.name, v.value);
    w.end_object();
    w.end_object();
}

// Absolute offsets inside SharedMemoryLayout, computed by pointer arithmetic on a real
// object (independent of the per-struct offsetof data, and cross-checked against it).
SharedMemoryLayout g_layout;

struct AbsRec {
    std::string path;
    size_t offset;
    size_t size;
};

std::vector<AbsRec> absolute_offsets() {
    std::vector<AbsRec> v;
    const char* base = reinterpret_cast<const char*>(&g_layout);
#define LP_ABS(expr) \
    v.push_back({#expr, static_cast<size_t>(reinterpret_cast<const char*>(&(g_layout.expr)) - base), sizeof(g_layout.expr)})
    LP_ABS(data);
    LP_ABS(data.generic);
    LP_ABS(data.generic.events);
    LP_ABS(data.generic.gameVersion);
    LP_ABS(data.generic.FFBTorque);
    LP_ABS(data.generic.appInfo);
    LP_ABS(data.paths);
    LP_ABS(data.paths.userData);
    LP_ABS(data.paths.customVariables);
    LP_ABS(data.paths.stewardResults);
    LP_ABS(data.paths.playerProfile);
    LP_ABS(data.paths.pluginsFolder);
    LP_ABS(data.scoring);
    LP_ABS(data.scoring.scoringInfo);
    LP_ABS(data.scoring.scoringInfo.mNumVehicles);
    LP_ABS(data.scoring.scoringInfo.mVehicle);
    LP_ABS(data.scoring.scoringStreamSize);
    LP_ABS(data.scoring.vehScoringInfo);
    LP_ABS(data.scoring.vehScoringInfo[1]);
    LP_ABS(data.scoring.vehScoringInfo[103]);
    LP_ABS(data.scoring.scoringStream);
    LP_ABS(data.telemetry);
    LP_ABS(data.telemetry.activeVehicles);
    LP_ABS(data.telemetry.playerVehicleIdx);
    LP_ABS(data.telemetry.playerHasVehicle);
    LP_ABS(data.telemetry.telemInfo);
    LP_ABS(data.telemetry.telemInfo[0].mDeltaTime);
    LP_ABS(data.telemetry.telemInfo[0].mWheel);
    LP_ABS(data.telemetry.telemInfo[0].mWheel[1]);
    LP_ABS(data.telemetry.telemInfo[1]);
    LP_ABS(data.telemetry.telemInfo[103]);
#undef LP_ABS
    return v;
}

std::string make_asserts(const std::vector<StructRec>& structs) {
    std::string o;
    o += "// Generated by tools/layout-probe/probe.cpp --emit-asserts. Do not edit by hand.\n";
    o += "// Compile against the installed LMU SDK headers (include path only; never copy them).\n";
    o += "// If this translation unit compiles, the compiler lays out every probed struct exactly\n";
    o += "// as recorded in docs/research/lmu_abi_layout.json.\n";
    o += "#include <cstddef>\n#ifndef NOMINMAX\n#define NOMINMAX\n#endif\n#include <windows.h>\n";
    o += "#if defined(_MSC_VER) && !defined(__clang__)\n#pragma warning(push)\n#pragma warning(disable : 4100 4201 4505)\n#endif\n";
    o += "#include \"SharedMemoryInterface.hpp\"\n";
    o += "#if defined(_MSC_VER) && !defined(__clang__)\n#pragma warning(pop)\n#endif\n\n";
    o += "static_assert(sizeof(void*) == 8, \"x64 only\");\n";
    o += "static_assert(MAX_PATH == " + std::to_string(MAX_PATH) + ", \"MAX_PATH\");\n";
    for (const StructRec& s : structs) {
        if (s.name.find("::") != std::string::npos) continue;  // private nested type: not nameable here
        o += "\n// " + s.name + "\n";
        o += "static_assert(sizeof(" + s.name + ") == " + std::to_string(s.size) + ", \"sizeof " + s.name + "\");\n";
        o += "static_assert(alignof(" + s.name + ") == " + std::to_string(s.align) + ", \"alignof " + s.name + "\");\n";
        for (const FieldRec& f : s.fields) {
            o += "static_assert(offsetof(" + s.name + ", " + f.name + ") == " + std::to_string(f.offset) +
                 " && sizeof(" + s.name + "::" + f.name + ") == " + std::to_string(f.size) + ", \"" + s.name +
                 "::" + f.name + "\");\n";
        }
    }
    o += "\n// enums\n";
    auto add_enum = [&o](const std::vector<EnumVal>& vals) {
        for (const EnumVal& v : vals)
            o += "static_assert(static_cast<long long>(" + std::string(v.expr) + ") == " + std::to_string(v.value) +
                 "LL, \"" + v.expr + "\");\n";
    };
    add_enum(sme_values());
    add_enum(vehicle_class_values());
    add_enum(vehicle_championship_values());
    o += "\nint main() { return 0; }\n";
    return o;
}

bool write_output(const char* path, const std::string& text) {
    if (!path) {
        std::fwrite(text.data(), 1, text.size(), stdout);
        return std::fflush(stdout) == 0;
    }
    std::FILE* f = std::fopen(path, "wb");
    if (!f) {
        std::fprintf(stderr, "probe: cannot open %s for writing\n", path);
        return false;
    }
    const bool ok = std::fwrite(text.data(), 1, text.size(), f) == text.size();
    return (std::fclose(f) == 0) && ok;
}

}  // namespace

int main(int argc, char** argv) {
    const char* out_path = nullptr;
    bool emit_asserts = false;
    for (int i = 1; i < argc; ++i) {
        if (std::strcmp(argv[i], "--out") == 0 && i + 1 < argc) {
            out_path = argv[++i];
        } else if (std::strcmp(argv[i], "--emit-asserts") == 0) {
            emit_asserts = true;
        } else {
            std::fprintf(stderr, "usage: probe.exe [--out <file>] [--emit-asserts]\n");
            return 1;
        }
    }

    // Keep the SDK's static helper referenced (avoids an "unreferenced static function" warning).
    volatile auto keep_copy_helper = &CopySharedMemoryObj;
    (void)keep_copy_helper;

    std::vector<StructRec> structs = {
        probe_SharedMemoryLayout(),     probe_SharedMemoryObjectOut(), probe_SharedMemoryGeneric(),
        probe_ApplicationStateV01(),    probe_SharedMemoryPathData(),  probe_SharedMemoryScoringData(),
        probe_ScoringInfoV01(),         probe_VehicleScoringInfoV01(), probe_SharedMemoryTelemetryData(),
        probe_TelemInfoV01(),           probe_TelemWheelV01(),         probe_TelemVect3(),
        probe_LockData(),
    };
    std::vector<std::string> global_errors;
    for (StructRec& s : structs) {
        analyze(s);
        for (const std::string& e : s.errors) global_errors.push_back(s.name + ": " + e);
    }

    const std::vector<AbsRec> abs = absolute_offsets();
    // Cross-check pointer arithmetic against summed offsetof values for the key paths.
    {
        const size_t telem_sum = offsetof(SharedMemoryLayout, data) + offsetof(SharedMemoryObjectOut, telemetry) +
                                 offsetof(SharedMemoryTelemetryData, telemInfo);
        const size_t veh_sum = offsetof(SharedMemoryLayout, data) + offsetof(SharedMemoryObjectOut, scoring) +
                               offsetof(SharedMemoryScoringData, vehScoringInfo);
        for (const AbsRec& a : abs) {
            if (a.path == "data.telemetry.telemInfo" && a.offset != telem_sum)
                global_errors.push_back("abs offset of data.telemetry.telemInfo disagrees with offsetof sum");
            if (a.path == "data.scoring.vehScoringInfo" && a.offset != veh_sum)
                global_errors.push_back("abs offset of data.scoring.vehScoringInfo disagrees with offsetof sum");
            if (a.path == "data.telemetry.telemInfo[103]" &&
                a.offset + a.size + 0 > sizeof(SharedMemoryLayout))
                global_errors.push_back("telemInfo[103] extends past sizeof(SharedMemoryLayout)");
        }
        if (sizeof(SharedMemoryLayout) != sizeof(SharedMemoryObjectOut))
            global_errors.push_back("sizeof(SharedMemoryLayout) != sizeof(SharedMemoryObjectOut)");
    }

    if (emit_asserts) {
        return write_output(out_path, make_asserts(structs)) ? (global_errors.empty() ? 0 : 2) : 1;
    }

    JsonWriter w;
    w.begin_object();
    w.str("schema", "lmu-abi-layout/1");
    w.str("description",
          "sizeof/alignof/offsetof facts for the LMU shared-memory SDK structs, produced by compiling "
          "tools/layout-probe/probe.cpp against the installed SDK headers (headers are referenced, never copied).");

    w.begin_object("generator");
#if defined(__clang__)
    w.str("compiler", "clang");
    w.str("compiler_version", __clang_version__);
#elif defined(_MSC_VER)
    w.str("compiler", "msvc");
    w.num("compiler_version", static_cast<unsigned long long>(_MSC_FULL_VER));
#elif defined(__GNUC__)
    w.str("compiler", "gcc");
    w.str("compiler_version", __VERSION__);
#else
    w.str("compiler", "unknown");
#endif
#if defined(__MINGW64__)
    w.str("target_abi", "x86_64-w64-mingw32 (MinGW-w64, Windows LLP64 data model)");
#elif defined(_MSC_VER)
    w.str("target_abi", "x86_64-pc-windows-msvc");
#else
    w.str("target_abi", "unknown");
#endif
#if defined(_MSVC_LANG)
    w.num("cplusplus", static_cast<unsigned long long>(_MSVC_LANG));
#else
    w.num("cplusplus", static_cast<unsigned long long>(__cplusplus));
#endif
    w.end_object();

    w.begin_object("target");
    w.num("sizeof_pointer", sizeof(void*));
    w.num("sizeof_size_t", sizeof(size_t));
    w.num("sizeof_HWND", sizeof(HWND));
    w.num("sizeof_bool", sizeof(bool));
    w.num("sizeof_short", sizeof(short));
    w.num("sizeof_int", sizeof(int));
    w.num("sizeof_long", sizeof(long));
    w.num("sizeof_win32_LONG", sizeof(LONG));  // distinct name: some JSON readers fold key case
    w.num("sizeof_unsigned_long", sizeof(unsigned long));
    w.num("sizeof_long_long", sizeof(long long));
    w.num("sizeof_float", sizeof(float));
    w.num("sizeof_float_t", sizeof(float_t));
    w.num("sizeof_double", sizeof(double));
    w.boolean("char_is_signed", std::is_signed_v<char>);
    w.num("default_pack_for_8byte_members", kDefaultPack);
    w.num("internals_plugin_pack", kInternalsPluginPack);
    w.end_object();

    w.begin_object("constants");
    w.num("MAX_PATH", MAX_PATH);
    w.num("SME_MAX", SME_MAX);
    w.num("scoring_vehicle_slots", std::extent_v<decltype(SharedMemoryScoringData::vehScoringInfo)>);
    w.num("telemetry_vehicle_slots", std::extent_v<decltype(SharedMemoryTelemetryData::telemInfo)>);
    w.num("scoring_stream_capacity", std::extent_v<decltype(SharedMemoryScoringData::scoringStream)>);
    w.begin_object("names");
    w.begin_object("LMU_SHARED_MEMORY_FILE", true);
    w.str("value", LMU_SHARED_MEMORY_FILE);
    w.str("source", "macro in SharedMemoryInterface.hpp");
    w.str("kind", "file mapping (game-created; clients OpenFileMapping)");
    w.end_object();
    w.begin_object("LMU_SHARED_MEMORY_HOLD_EVENT", true);
    w.str("value", LMU_SHARED_MEMORY_HOLD_EVENT);
    w.str("source", "macro in SharedMemoryInterface.hpp");
    w.str("kind", "named event, opened with OpenEventA(SYNCHRONIZE); waited on first");
    w.end_object();
    w.begin_object("LMU_SHARED_MEMORY_DATA_EVENT", true);
    w.str("value", LMU_SHARED_MEMORY_DATA_EVENT);
    w.str("source", "macro in SharedMemoryInterface.hpp");
    w.str("kind", "named event, opened with OpenEventA(SYNCHRONIZE); waited on second");
    w.end_object();
    w.begin_object("lock_mapping", true);
    w.str("value", "LMU_SharedMemoryLockData");
    w.str("source", "string literal in SharedMemoryLock::Init (hand-transcribed; not a macro)");
    w.str("kind", "file mapping created-or-opened with CreateFileMappingA, size sizeof(LockData)");
    w.end_object();
    w.begin_object("lock_event", true);
    w.str("value", "LMU_SharedMemoryLockEvent");
    w.str("source", "string literal in SharedMemoryLock::Init (hand-transcribed; not a macro)");
    w.str("kind", "named event created-or-opened with CreateEventA(auto-reset, initially non-signaled)");
    w.end_object();
    w.end_object();
    w.end_object();

    w.begin_object("enums");
    emit_enum<SharedMemoryEvent>(w, "SharedMemoryEvent", kSMI, sme_values());
    emit_enum<IP_VehicleClass>(w, "IP_VehicleClass", kIP, vehicle_class_values());
    emit_enum<IP_VehicleChampionship>(w, "IP_VehicleChampionship", kIP, vehicle_championship_values());
    w.end_object();

    w.begin_object("structs");
    for (const StructRec& s : structs) {
        w.begin_object(s.name);
        w.str("header", s.header);
        w.num("sizeof", s.size);
        w.num("alignof", s.align);
        w.num("pack", s.pack);
        w.str("pack_source", s.pack_source);
        w.num("field_count", s.fields.size());
        w.num("sum_field_sizes", s.sum_field_sizes);
        w.num("covered_bytes", s.covered_bytes);
        w.num("padding_bytes", s.size - s.covered_bytes);
        w.num("fields_below_natural_alignment", s.fields_below_natural_alignment);
        w.boolean("ok", s.errors.empty());
        w.begin_array("padding");
        for (const HoleRec& h : s.holes) {
            w.begin_object("", true);
            w.num("offset", h.offset);
            w.num("size", h.size);
            w.str("kind", h.tail ? "tail" : "interior");
            w.str("after", h.after);
            if (!h.tail) w.str("before", h.before);
            w.boolean("explained_by_alignment", h.explained);
            w.end_object();
        }
        w.end_array();
        w.begin_array("fields");
        for (const FieldRec& f : s.fields) {
            w.begin_object("", true);
            w.str("name", f.name);
            w.num("offset", f.offset);
            w.num("size", f.size);
            w.str("type", f.type);
            w.str("kind", f.kind);
            w.num("elem_size", f.elem_size);
            w.begin_array("dims", true);
            for (size_t d : f.dims) w.num("", d);
            w.end_array();
            w.num("align", f.align);
            w.boolean("naturally_aligned", f.offset % f.align == 0);
            if (f.union_group >= 0) {
                w.num("union_group", static_cast<unsigned long long>(f.union_group));
                w.num("union_alt", static_cast<unsigned long long>(f.union_alt));
            }
            w.end_object();
        }
        w.end_array();
        if (!s.errors.empty()) {
            w.begin_array("errors");
            for (const std::string& e : s.errors) w.str("", e);
            w.end_array();
        }
        w.end_object();
    }
    w.end_object();

    w.begin_object("absolute_offsets");
    w.str("base", "SharedMemoryLayout (start of the LMU_Data mapping view)");
    w.num("sizeof_SharedMemoryLayout", sizeof(SharedMemoryLayout));
    w.num("pages_4k", (sizeof(SharedMemoryLayout) + 4095) / 4096);
    w.begin_array("paths");
    for (const AbsRec& a : abs) {
        w.begin_object("", true);
        w.str("path", a.path);
        w.num("offset", a.offset);
        w.num("size", a.size);
        w.num("offset_mod_8", a.offset % 8);
        w.end_object();
    }
    w.end_array();
    w.end_object();

    w.begin_object("checks");
    w.boolean("all_ok", global_errors.empty());
    w.begin_array("errors");
    for (const std::string& e : global_errors) w.str("", e);
    w.end_array();
    w.end_object();

    w.end_object();
    w.out += '\n';

    if (!write_output(out_path, w.out)) return 1;
    return global_errors.empty() ? 0 : 2;
}
