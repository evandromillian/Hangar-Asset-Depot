#pragma once

#include "CoreMinimal.h"
#include "Dom/JsonObject.h"

// The subset of Hangar's JSON (hangar-core: bridge.rs, preview.rs) the plugin reads.

/** `event: import` of the bridge stream. */
struct FHangarImport
{
	FString DeliveryId;
	FString ProjectSlug;
	FString ProjectName;
	FString AssetId;
	FString AssetSlug;
	FString AssetName;
	FString Kind;
	FString VersionId;
	int32 VersionNumber = 0;
	FString SentBy;

	static bool Parse(const TSharedPtr<FJsonObject>& Json, FHangarImport& Out);
};

struct FHangarMaterialTexture
{
	/** "base_color", "normal", "orm", ... (hangar_core::TextureSlot). */
	FString Slot;
	/** Version-relative path; "@<set slug>/..." for texture-set images. Empty when missing. */
	FString Path;
};

struct FHangarMaterial
{
	FString Name;
	/** glTF-style linear factor. */
	FLinearColor BaseColor = FLinearColor::White;
	float Metallic = 0.f;
	float Roughness = 1.f;
	TArray<FHangarMaterialTexture> Textures;
	/** "OPAQUE", "MASK", "BLEND" or empty (older manifests). */
	FString AlphaMode;
	TOptional<float> AlphaCutoff;

	const FHangarMaterialTexture* Find(const TCHAR* Slot) const;
};

struct FHangarTextureOverride
{
	FString Material;
	FString Slot;
	FString Path;
};

struct FHangarVariant
{
	FString Name;
	bool bIsDefault = false;
	TArray<FHangarTextureOverride> Overrides;
};

struct FHangarTextureSetUse
{
	FString AssetId;
	FString Name;
	/** "@slug" */
	FString Prefix;
	FString VersionId;
	int32 VersionNumber = 0;
	FString Error;
};

struct FHangarManifest
{
	FString Kind;
	FString PrimaryFile;
	bool bHasSkin = false;
	TArray<FHangarMaterial> Materials;
	TArray<FHangarVariant> Variants;
	TArray<FHangarTextureSetUse> TextureSets;

	static void Parse(const TSharedPtr<FJsonObject>& Json, FHangarManifest& Out);
};

struct FHangarFile
{
	FString Path;
	FString Hash;
	int64 Size = 0;
};

/** GET /api/versions/{id}/bundle */
struct FHangarBundle
{
	int32 VersionNumber = 0;
	FHangarManifest Manifest;
	bool bHasManifest = false;
	TArray<FHangarFile> Files;

	static bool Parse(const TSharedPtr<FJsonObject>& Json, FHangarBundle& Out);
};

namespace Hangar
{
	TSharedPtr<FJsonObject> ParseObject(const FString& Text);
	FString ToJson(const TSharedRef<FJsonObject>& Object);
}
