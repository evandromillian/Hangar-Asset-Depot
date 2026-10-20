#include "HangarMaterials.h"

#include "AssetRegistry/AssetRegistryModule.h"
#include "AssetToolsModule.h"
#include "Engine/Texture2D.h"
#include "Factories/MaterialFactoryNew.h"
#include "Factories/MaterialInstanceConstantFactoryNew.h"
#include "HangarApi.h"
#include "HangarTypes.h"
#include "MaterialEditingLibrary.h"
#include "Materials/Material.h"
#include "Materials/MaterialExpressionDotProduct.h"
#include "Materials/MaterialExpressionLinearInterpolate.h"
#include "Materials/MaterialExpressionMultiply.h"
#include "Materials/MaterialExpressionScalarParameter.h"
#include "Materials/MaterialExpressionTextureSampleParameter2D.h"
#include "Materials/MaterialExpressionVectorParameter.h"
#include "Materials/MaterialInstanceConstant.h"
#include "TextureCompiler.h"
#include "UObject/Package.h"

namespace
{
	// Bump to rebuild the masters of existing projects when the graph changes.
	constexpr const TCHAR* MasterVersion = TEXT("1");

	const TCHAR* MasterNames[] = {TEXT("M_HangarOpaque"), TEXT("M_HangarMasked"), TEXT("M_HangarTranslucent")};

	FString MaterialsPath(const FString& Root)
	{
		return Root / TEXT("_Materials");
	}

	template <typename T>
	T* LoadAsset(const FString& PackagePath, const FString& Name)
	{
		return LoadObject<T>(nullptr, *FString::Printf(TEXT("%s/%s.%s"), *PackagePath, *Name, *Name), nullptr, LOAD_NoWarn | LOAD_Quiet);
	}

	UTexture2D* DefaultTexture(const FString& Path, const FString& Name, FColor Color, TextureCompressionSettings Compression, bool bSRGB)
	{
		if (UTexture2D* Existing = LoadAsset<UTexture2D>(Path, Name))
		{
			return Existing;
		}
		UPackage* Package = CreatePackage(*(Path / Name));
		UTexture2D* Texture = NewObject<UTexture2D>(Package, *Name, RF_Public | RF_Standalone);
		TArray<FColor> Pixels;
		Pixels.Init(Color, 4 * 4);
		Texture->Source.Init(4, 4, 1, 1, TSF_BGRA8, reinterpret_cast<const uint8*>(Pixels.GetData()));
		Texture->CompressionSettings = Compression;
		Texture->SRGB = bSRGB;
		if (Compression == TC_Normalmap)
		{
			Texture->LODGroup = TEXTUREGROUP_WorldNormalMap;
		}
		Texture->PostEditChange();
		FAssetRegistryModule::AssetCreated(Texture);
		Package->MarkPackageDirty();
		return Texture;
	}

	/** Builds a master material graph with UMaterialEditingLibrary. */
	struct FGraph
	{
		UMaterial* Material;

		template <typename T>
		T* Add(int32 X, int32 Y)
		{
			return Cast<T>(UMaterialEditingLibrary::CreateMaterialExpression(Material, T::StaticClass(), X, Y));
		}

		UMaterialExpressionTextureSampleParameter2D* Texture(FName Name, UTexture* Default, EMaterialSamplerType Type, int32 Y)
		{
			auto* E = Add<UMaterialExpressionTextureSampleParameter2D>(-900, Y);
			E->ParameterName = Name;
			E->Texture = Default;
			E->SamplerType = Type;
			E->Group = TEXT("Hangar");
			return E;
		}

		UMaterialExpressionVectorParameter* Vector(FName Name, FLinearColor Default, int32 Y)
		{
			auto* E = Add<UMaterialExpressionVectorParameter>(-900, Y);
			E->ParameterName = Name;
			E->DefaultValue = Default;
			E->Group = TEXT("Hangar");
			return E;
		}

		UMaterialExpressionScalarParameter* Scalar(FName Name, float Default, int32 Y)
		{
			auto* E = Add<UMaterialExpressionScalarParameter>(-900, Y);
			E->ParameterName = Name;
			E->DefaultValue = Default;
			E->Group = TEXT("Hangar");
			return E;
		}

		void Link(UMaterialExpression* From, const TCHAR* Output, UMaterialExpression* To, const TCHAR* Input)
		{
			UMaterialEditingLibrary::ConnectMaterialExpressions(From, Output, To, Input);
		}

		UMaterialExpression* Multiply(UMaterialExpression* A, const TCHAR* AOut, UMaterialExpression* B, const TCHAR* BOut, int32 Y)
		{
			auto* E = Add<UMaterialExpressionMultiply>(-300, Y);
			Link(A, AOut, E, TEXT("A"));
			Link(B, BOut, E, TEXT("B"));
			return E;
		}

		/** Texture channel picked by an RGBA mask parameter: dot(texture, mask). */
		UMaterialExpression* Channel(UMaterialExpression* Texture, UMaterialExpression* Mask, int32 Y)
		{
			auto* E = Add<UMaterialExpressionDotProduct>(-550, Y);
			Link(Texture, TEXT("RGBA"), E, TEXT("A"));
			Link(Mask, TEXT("RGBA"), E, TEXT("B"));
			return E;
		}
	};

	void BuildGraph(UMaterial* Material, EBlendMode Blend, UTexture* White, UTexture* WhiteMask, UTexture* FlatNormal)
	{
		FGraph G{Material};
		const FLinearColor Red(1, 0, 0, 0);

		auto* BaseTex = G.Texture(TEXT("BaseColor"), White, SAMPLERTYPE_Color, 0);
		auto* BaseFactor = G.Vector(TEXT("BaseColorFactor"), FLinearColor::White, 220);
		UMaterialEditingLibrary::ConnectMaterialProperty(G.Multiply(BaseTex, TEXT("RGB"), BaseFactor, TEXT("RGB"), 0), TEXT(""), MP_BaseColor);

		auto* NormalTex = G.Texture(TEXT("Normal"), FlatNormal, SAMPLERTYPE_Normal, 400);
		UMaterialEditingLibrary::ConnectMaterialProperty(NormalTex, TEXT("RGB"), MP_Normal);

		auto* RoughTex = G.Texture(TEXT("Roughness"), WhiteMask, SAMPLERTYPE_Masks, 640);
		auto* RoughMask = G.Vector(TEXT("RoughnessChannel"), Red, 860);
		auto* RoughFactor = G.Scalar(TEXT("RoughnessFactor"), 1.f, 1000);
		UMaterialEditingLibrary::ConnectMaterialProperty(
			G.Multiply(G.Channel(RoughTex, RoughMask, 700), TEXT(""), RoughFactor, TEXT(""), 700), TEXT(""), MP_Roughness);

		auto* MetalTex = G.Texture(TEXT("Metallic"), WhiteMask, SAMPLERTYPE_Masks, 1120);
		auto* MetalMask = G.Vector(TEXT("MetallicChannel"), Red, 1340);
		auto* MetalFactor = G.Scalar(TEXT("MetallicFactor"), 0.f, 1480);
		UMaterialEditingLibrary::ConnectMaterialProperty(
			G.Multiply(G.Channel(MetalTex, MetalMask, 1180), TEXT(""), MetalFactor, TEXT(""), 1180), TEXT(""), MP_Metallic);

		auto* AoTex = G.Texture(TEXT("Occlusion"), WhiteMask, SAMPLERTYPE_Masks, 1600);
		auto* AoMask = G.Vector(TEXT("OcclusionChannel"), Red, 1820);
		UMaterialEditingLibrary::ConnectMaterialProperty(G.Channel(AoTex, AoMask, 1660), TEXT(""), MP_AmbientOcclusion);

		auto* EmissiveTex = G.Texture(TEXT("Emissive"), White, SAMPLERTYPE_Color, 1960);
		auto* EmissiveFactor = G.Vector(TEXT("EmissiveFactor"), FLinearColor::Black, 2180);
		UMaterialEditingLibrary::ConnectMaterialProperty(
			G.Multiply(EmissiveTex, TEXT("RGB"), EmissiveFactor, TEXT("RGB"), 1960), TEXT(""), MP_EmissiveColor);

		// Reflectance convention (0.5 = 4 % F0), the same as Unreal's Specular input.
		auto* SpecTex = G.Texture(TEXT("Specular"), White, SAMPLERTYPE_Color, 2320);
		auto* SpecFactor = G.Scalar(TEXT("SpecularFactor"), 0.5f, 2540);
		UMaterialEditingLibrary::ConnectMaterialProperty(
			G.Multiply(SpecTex, TEXT("R"), SpecFactor, TEXT(""), 2320), TEXT(""), MP_Specular);

		Material->BlendMode = Blend;
		if (Blend == BLEND_Opaque)
		{
			return;
		}
		// Alpha: the base colour's, or an Opacity map's channel when UseOpacityMap = 1.
		auto* OpacityTex = G.Texture(TEXT("Opacity"), WhiteMask, SAMPLERTYPE_Masks, 2660);
		auto* OpacityMask = G.Vector(TEXT("OpacityChannel"), Red, 2880);
		auto* UseOpacity = G.Scalar(TEXT("UseOpacityMap"), 0.f, 3020);
		auto* Pick = G.Add<UMaterialExpressionLinearInterpolate>(-300, 2660);
		G.Link(BaseTex, TEXT("A"), Pick, TEXT("A"));
		G.Link(G.Channel(OpacityTex, OpacityMask, 2720), TEXT(""), Pick, TEXT("B"));
		G.Link(UseOpacity, TEXT(""), Pick, TEXT("Alpha"));
		UMaterialExpression* Alpha = G.Multiply(Pick, TEXT(""), BaseFactor, TEXT("A"), 2800);
		if (Blend == BLEND_Masked)
		{
			UMaterialEditingLibrary::ConnectMaterialProperty(Alpha, TEXT(""), MP_OpacityMask);
		}
		else
		{
			UMaterialEditingLibrary::ConnectMaterialProperty(Alpha, TEXT(""), MP_Opacity);
			Material->TranslucencyLightingMode = TLM_SurfacePerPixelLighting;
		}
	}

	HangarMaterials::ETextureKind KindOf(const UTexture2D* Texture)
	{
		switch (Texture->CompressionSettings)
		{
		case TC_Normalmap:
			return HangarMaterials::ETextureKind::Normal;
		case TC_Masks:
		case TC_Grayscale:
			return HangarMaterials::ETextureKind::Linear;
		default:
			return Texture->SRGB ? HangarMaterials::ETextureKind::Color : HangarMaterials::ETextureKind::Linear;
		}
	}
}

HangarMaterials::ETextureKind HangarMaterials::KindForSlot(const FString& Slot)
{
	if (Slot == TEXT("normal"))
	{
		return ETextureKind::Normal;
	}
	if (Slot == TEXT("base_color") || Slot == TEXT("emissive") || Slot == TEXT("specular"))
	{
		return ETextureKind::Color;
	}
	return ETextureKind::Linear;
}

bool HangarMaterials::EnsureMasters(const FString& Root, FString& OutError)
{
	const FString Path = MaterialsPath(Root);
	bool bAllCurrent = true;
	for (const TCHAR* Name : MasterNames)
	{
		UMaterial* Existing = LoadAsset<UMaterial>(Path, Name);
		const FString* Version = Existing ? Existing->GetPackage()->GetMetaData().FindValue(Existing, TEXT("HangarMasterVersion")) : nullptr;
		bAllCurrent &= Version && *Version == MasterVersion;
	}
	if (bAllCurrent)
	{
		return true;
	}

	UTexture* White = DefaultTexture(Path, TEXT("T_HangarWhite"), FColor::White, TC_Default, true);
	UTexture* WhiteMask = DefaultTexture(Path, TEXT("T_HangarWhiteMask"), FColor::White, TC_Masks, false);
	UTexture* FlatNormal = DefaultTexture(Path, TEXT("T_HangarFlatNormal"), FColor(128, 128, 255), TC_Normalmap, false);

	const EBlendMode Blends[] = {BLEND_Opaque, BLEND_Masked, BLEND_Translucent};
	IAssetTools& AssetTools = FModuleManager::LoadModuleChecked<FAssetToolsModule>("AssetTools").Get();
	for (int32 i = 0; i < UE_ARRAY_COUNT(MasterNames); ++i)
	{
		UMaterial* Material = LoadAsset<UMaterial>(Path, MasterNames[i]);
		if (Material)
		{
			// Older graph: rebuild it in place, so instances keep their parent.
			UMaterialEditingLibrary::DeleteAllMaterialExpressions(Material);
		}
		else
		{
			UMaterialFactoryNew* Factory = NewObject<UMaterialFactoryNew>();
			Material = Cast<UMaterial>(AssetTools.CreateAsset(MasterNames[i], Path, UMaterial::StaticClass(), Factory));
		}
		if (!Material)
		{
			OutError = FString::Printf(TEXT("não foi possível criar %s/%s"), *Path, MasterNames[i]);
			return false;
		}
		BuildGraph(Material, Blends[i], White, WhiteMask, FlatNormal);
		UMaterialEditingLibrary::RecompileMaterial(Material);
		Material->GetPackage()->GetMetaData().SetValue(Material, TEXT("HangarMasterVersion"), MasterVersion);
		Material->MarkPackageDirty();
	}
	return true;
}

UMaterialInstanceConstant* HangarMaterials::BuildInstance(const FString& Root, const FString& PackagePath, const FString& Name,
	const FHangarMaterial& Material, const TMap<FString, UTexture2D*>& Textures, const TMap<FString, FString>& Overrides,
	TArray<FString>& OutWarnings)
{
	const FString Mode = Material.AlphaMode.ToUpper();
	const TCHAR* MasterName = Mode == TEXT("MASK") ? MasterNames[1] : Mode == TEXT("BLEND") ? MasterNames[2] : MasterNames[0];
	UMaterial* Master = LoadAsset<UMaterial>(MaterialsPath(Root), MasterName);
	if (!Master)
	{
		OutWarnings.Add(FString::Printf(TEXT("material mestre %s não encontrado"), MasterName));
		return nullptr;
	}

	UMaterialInstanceConstant* Instance = LoadAsset<UMaterialInstanceConstant>(PackagePath, Name);
	if (Instance)
	{
		Instance->SetParentEditorOnly(Master);
		Instance->ClearParameterValuesEditorOnly();
	}
	else
	{
		UMaterialInstanceConstantFactoryNew* Factory = NewObject<UMaterialInstanceConstantFactoryNew>();
		Factory->InitialParent = Master;
		IAssetTools& AssetTools = FModuleManager::LoadModuleChecked<FAssetToolsModule>("AssetTools").Get();
		Instance = Cast<UMaterialInstanceConstant>(AssetTools.CreateAsset(Name, PackagePath, UMaterialInstanceConstant::StaticClass(), Factory));
	}
	if (!Instance)
	{
		OutWarnings.Add(FString::Printf(TEXT("não foi possível criar %s/%s"), *PackagePath, *Name));
		return nullptr;
	}

	auto PathOf = [&](const TCHAR* Slot) -> FString
	{
		if (const FString* Over = Overrides.Find(Slot))
		{
			return *Over;
		}
		const FHangarMaterialTexture* Tex = Material.Find(Slot);
		return Tex ? Tex->Path : FString();
	};
	auto Bind = [&](const TCHAR* Param, const FString& Path, ETextureKind Kind) -> UTexture2D*
	{
		UTexture2D* Texture = Path.IsEmpty() ? nullptr : Textures.FindRef(Path);
		if (!Texture)
		{
			return nullptr;
		}
		if (KindOf(Texture) != Kind)
		{
			OutWarnings.Add(FString::Printf(TEXT("%s: %s é usada em outro slot com outra compressão; ignorada em %s"),
				*Material.Name, *Path, Param));
			return nullptr;
		}
		UMaterialEditingLibrary::SetMaterialInstanceTextureParameterValue(Instance, Param, Texture);
		return Texture;
	};
	auto SetScalar = [&](const TCHAR* Param, float Value) { UMaterialEditingLibrary::SetMaterialInstanceScalarParameterValue(Instance, Param, Value); };
	auto SetVector = [&](const TCHAR* Param, FLinearColor Value) { UMaterialEditingLibrary::SetMaterialInstanceVectorParameterValue(Instance, Param, Value); };
	const FLinearColor R(1, 0, 0, 0), G(0, 1, 0, 0), B(0, 0, 1, 0), A(0, 0, 0, 1);

	// A texture replaces its factor (the GLB export's rule).
	const FString BasePath = PathOf(TEXT("base_color"));
	const bool bBase = Bind(TEXT("BaseColor"), BasePath, ETextureKind::Color) != nullptr;
	SetVector(TEXT("BaseColorFactor"), bBase ? FLinearColor(1, 1, 1, Material.BaseColor.A) : Material.BaseColor);
	Bind(TEXT("Normal"), PathOf(TEXT("normal")), ETextureKind::Normal);

	float Roughness = Material.Roughness;
	float Metallic = Material.Metallic;
	const FString Orm = PathOf(TEXT("orm"));
	const FString MetalRough = PathOf(TEXT("metallic_roughness"));
	if (!Orm.IsEmpty() && Bind(TEXT("Roughness"), Orm, ETextureKind::Linear))
	{
		Bind(TEXT("Metallic"), Orm, ETextureKind::Linear);
		Bind(TEXT("Occlusion"), Orm, ETextureKind::Linear);
		SetVector(TEXT("RoughnessChannel"), G);
		SetVector(TEXT("MetallicChannel"), B);
		SetVector(TEXT("OcclusionChannel"), R);
		Roughness = Metallic = 1.f;
	}
	else
	{
		if (!MetalRough.IsEmpty() && Bind(TEXT("Roughness"), MetalRough, ETextureKind::Linear))
		{
			Bind(TEXT("Metallic"), MetalRough, ETextureKind::Linear);
			SetVector(TEXT("RoughnessChannel"), G);
			SetVector(TEXT("MetallicChannel"), B);
			Roughness = Metallic = 1.f;
		}
		else
		{
			if (Bind(TEXT("Roughness"), PathOf(TEXT("roughness")), ETextureKind::Linear))
			{
				Roughness = 1.f;
			}
			if (Bind(TEXT("Metallic"), PathOf(TEXT("metallic")), ETextureKind::Linear))
			{
				Metallic = 1.f;
			}
		}
		Bind(TEXT("Occlusion"), PathOf(TEXT("occlusion")), ETextureKind::Linear);
	}
	SetScalar(TEXT("RoughnessFactor"), Roughness);
	SetScalar(TEXT("MetallicFactor"), Metallic);

	if (Bind(TEXT("Emissive"), PathOf(TEXT("emissive")), ETextureKind::Color))
	{
		SetVector(TEXT("EmissiveFactor"), FLinearColor::White);
	}
	if (Bind(TEXT("Specular"), PathOf(TEXT("specular")), ETextureKind::Color))
	{
		SetScalar(TEXT("SpecularFactor"), 1.f);
	}

	// An Opacity map drives alpha from its own alpha channel when it has one, else
	// from red (images::merge_alpha). The same file as the base colour = its alpha.
	const FString OpacityPath = PathOf(TEXT("opacity"));
	if (Mode != TEXT("OPAQUE") && !Mode.IsEmpty() && OpacityPath != BasePath)
	{
		if (UTexture2D* Opacity = Bind(TEXT("Opacity"), OpacityPath, ETextureKind::Linear))
		{
			// Whether it has alpha is known once the platform data is built.
			UTexture* AsTexture = Opacity;
			FTextureCompilingManager::Get().FinishCompilation(MakeArrayView(&AsTexture, 1));
			SetScalar(TEXT("UseOpacityMap"), 1.f);
			SetVector(TEXT("OpacityChannel"), Opacity->HasAlphaChannel() ? A : R);
		}
	}
	if (Mode == TEXT("MASK"))
	{
		Instance->BasePropertyOverrides.bOverride_OpacityMaskClipValue = true;
		Instance->BasePropertyOverrides.OpacityMaskClipValue = Material.AlphaCutoff.Get(0.5f);
	}

	Instance->PostEditChange();
	Instance->MarkPackageDirty();
	return Instance;
}
