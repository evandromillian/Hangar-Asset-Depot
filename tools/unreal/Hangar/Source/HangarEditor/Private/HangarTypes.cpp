#include "HangarTypes.h"

#include "Policies/CondensedJsonPrintPolicy.h"
#include "Serialization/JsonReader.h"
#include "Serialization/JsonSerializer.h"
#include "Serialization/JsonWriter.h"

namespace
{
	FString Str(const TSharedPtr<FJsonObject>& Json, const TCHAR* Field)
	{
		FString Value;
		if (Json.IsValid())
		{
			Json->TryGetStringField(Field, Value);
		}
		return Value;
	}

	const TArray<TSharedPtr<FJsonValue>>& Array(const TSharedPtr<FJsonObject>& Json, const TCHAR* Field)
	{
		static const TArray<TSharedPtr<FJsonValue>> Empty;
		const TArray<TSharedPtr<FJsonValue>>* Values = nullptr;
		return Json.IsValid() && Json->TryGetArrayField(Field, Values) ? *Values : Empty;
	}

	TSharedPtr<FJsonObject> Object(const TSharedPtr<FJsonObject>& Json, const TCHAR* Field)
	{
		const TSharedPtr<FJsonObject>* Value = nullptr;
		return Json.IsValid() && Json->TryGetObjectField(Field, Value) ? *Value : nullptr;
	}
}

bool FHangarImport::Parse(const TSharedPtr<FJsonObject>& Json, FHangarImport& Out)
{
	if (!Json.IsValid())
	{
		return false;
	}
	Out.DeliveryId = Str(Json, TEXT("delivery_id"));
	Out.ProjectSlug = Str(Json, TEXT("project_slug"));
	Out.ProjectName = Str(Json, TEXT("project_name"));
	Out.AssetId = Str(Json, TEXT("asset_id"));
	Out.AssetSlug = Str(Json, TEXT("asset_slug"));
	Out.AssetName = Str(Json, TEXT("asset_name"));
	Out.Kind = Str(Json, TEXT("kind"));
	Out.VersionId = Str(Json, TEXT("version_id"));
	Json->TryGetNumberField(TEXT("version_number"), Out.VersionNumber);
	Out.SentBy = Str(Json, TEXT("sent_by"));
	return !Out.DeliveryId.IsEmpty() && !Out.VersionId.IsEmpty() && !Out.AssetSlug.IsEmpty();
}

const FHangarMaterialTexture* FHangarMaterial::Find(const TCHAR* Slot) const
{
	return Textures.FindByPredicate([Slot](const FHangarMaterialTexture& T) { return T.Slot == Slot && !T.Path.IsEmpty(); });
}

void FHangarManifest::Parse(const TSharedPtr<FJsonObject>& Json, FHangarManifest& Out)
{
	Out.Kind = Str(Json, TEXT("kind"));
	Out.PrimaryFile = Str(Json, TEXT("primary_file"));
	if (TSharedPtr<FJsonObject> Stats = Object(Json, TEXT("stats")))
	{
		Stats->TryGetBoolField(TEXT("has_skin"), Out.bHasSkin);
	}
	for (const TSharedPtr<FJsonValue>& Value : Array(Json, TEXT("materials")))
	{
		const TSharedPtr<FJsonObject> M = Value->AsObject();
		if (!M.IsValid())
		{
			continue;
		}
		FHangarMaterial& Mat = Out.Materials.AddDefaulted_GetRef();
		Mat.Name = Str(M, TEXT("name"));
		const TArray<TSharedPtr<FJsonValue>>& Color = Array(M, TEXT("base_color"));
		if (Color.Num() == 4)
		{
			Mat.BaseColor = FLinearColor(float(Color[0]->AsNumber()), float(Color[1]->AsNumber()), float(Color[2]->AsNumber()),
				float(Color[3]->AsNumber()));
		}
		double Number = 0;
		if (M->TryGetNumberField(TEXT("metallic"), Number))
		{
			Mat.Metallic = float(Number);
		}
		if (M->TryGetNumberField(TEXT("roughness"), Number))
		{
			Mat.Roughness = float(Number);
		}
		Mat.AlphaMode = Str(M, TEXT("alpha_mode"));
		if (M->TryGetNumberField(TEXT("alpha_cutoff"), Number))
		{
			Mat.AlphaCutoff = float(Number);
		}
		for (const TSharedPtr<FJsonValue>& T : Array(M, TEXT("textures")))
		{
			const TSharedPtr<FJsonObject> Tex = T->AsObject();
			if (Tex.IsValid())
			{
				Mat.Textures.Add({Str(Tex, TEXT("slot")), Str(Tex, TEXT("path"))});
			}
		}
	}
	for (const TSharedPtr<FJsonValue>& Value : Array(Json, TEXT("variants")))
	{
		const TSharedPtr<FJsonObject> V = Value->AsObject();
		if (!V.IsValid())
		{
			continue;
		}
		FHangarVariant& Variant = Out.Variants.AddDefaulted_GetRef();
		Variant.Name = Str(V, TEXT("name"));
		V->TryGetBoolField(TEXT("is_default"), Variant.bIsDefault);
		for (const TSharedPtr<FJsonValue>& O : Array(V, TEXT("overrides")))
		{
			const TSharedPtr<FJsonObject> Over = O->AsObject();
			if (Over.IsValid())
			{
				Variant.Overrides.Add({Str(Over, TEXT("material")), Str(Over, TEXT("slot")), Str(Over, TEXT("path"))});
			}
		}
	}
	for (const TSharedPtr<FJsonValue>& Value : Array(Json, TEXT("texture_sets")))
	{
		const TSharedPtr<FJsonObject> S = Value->AsObject();
		if (!S.IsValid())
		{
			continue;
		}
		FHangarTextureSetUse& Set = Out.TextureSets.AddDefaulted_GetRef();
		Set.AssetId = Str(S, TEXT("asset_id"));
		Set.Name = Str(S, TEXT("name"));
		Set.Prefix = Str(S, TEXT("prefix"));
		Set.VersionId = Str(S, TEXT("version_id"));
		S->TryGetNumberField(TEXT("version_number"), Set.VersionNumber);
		Set.Error = Str(S, TEXT("error"));
	}
}

bool FHangarBundle::Parse(const TSharedPtr<FJsonObject>& Json, FHangarBundle& Out)
{
	const TSharedPtr<FJsonObject> Version = Object(Json, TEXT("version"));
	if (!Version.IsValid())
	{
		return false;
	}
	Version->TryGetNumberField(TEXT("number"), Out.VersionNumber);
	if (const TSharedPtr<FJsonObject> Preview = Object(Version, TEXT("preview")))
	{
		FHangarManifest::Parse(Preview, Out.Manifest);
		Out.bHasManifest = true;
	}
	for (const TSharedPtr<FJsonValue>& Value : Array(Json, TEXT("files")))
	{
		const TSharedPtr<FJsonObject> F = Value->AsObject();
		if (!F.IsValid())
		{
			continue;
		}
		FHangarFile& File = Out.Files.AddDefaulted_GetRef();
		File.Path = Str(F, TEXT("path"));
		File.Hash = Str(F, TEXT("hash"));
		F->TryGetNumberField(TEXT("size"), File.Size);
	}
	return true;
}

TSharedPtr<FJsonObject> Hangar::ParseObject(const FString& Text)
{
	TSharedPtr<FJsonObject> Object;
	const TSharedRef<TJsonReader<>> Reader = TJsonReaderFactory<>::Create(Text);
	FJsonSerializer::Deserialize(Reader, Object);
	return Object;
}

FString Hangar::ToJson(const TSharedRef<FJsonObject>& Object)
{
	FString Out;
	const TSharedRef<TJsonWriter<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>> Writer =
		TJsonWriterFactory<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>::Create(&Out);
	FJsonSerializer::Serialize(Object, Writer);
	return Out;
}
