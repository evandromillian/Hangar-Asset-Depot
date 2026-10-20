#pragma once

#include "CoreMinimal.h"

struct FHangarMaterial;
class UMaterialInstanceConstant;
class UMaterialInterface;
class UTexture2D;

/**
 * Hangar's master materials (created once under <root>/_Materials) and the
 * material instances that rebuild a manifest material from its texture slots,
 * with the same factor rules as Hangar's GLB export: a texture replaces its
 * factor; ORM = AO (R) / roughness (G) / metallic (B); metal/rough = G / B.
 */
namespace HangarMaterials
{
	/** How a texture is sampled; decides its compression when imported. */
	enum class ETextureKind : uint8
	{
		Color,
		Linear,
		Normal,
	};

	ETextureKind KindForSlot(const FString& Slot);

	/** Creates the master materials and their default textures if missing. */
	bool EnsureMasters(const FString& Root, FString& OutError);

	/**
	 * Creates or updates the instance `PackagePath/Name` for `Material`. `Textures`
	 * maps version paths ("@set/…" included) to imported textures; `Overrides`
	 * (slot → path) replaces the material's own textures (skins).
	 */
	UMaterialInstanceConstant* BuildInstance(const FString& Root, const FString& PackagePath, const FString& Name,
		const FHangarMaterial& Material, const TMap<FString, UTexture2D*>& Textures,
		const TMap<FString, FString>& Overrides, TArray<FString>& OutWarnings);
}
