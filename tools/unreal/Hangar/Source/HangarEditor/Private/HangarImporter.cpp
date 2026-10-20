#include "HangarImporter.h"

#include "Algo/AnyOf.h"
#include "Animation/Skeleton.h"
#include "Async/Async.h"
#include "AssetImportTask.h"
#include "AssetToolsModule.h"
#include "Editor.h"
#include "Engine/SkeletalMesh.h"
#include "Engine/StaticMesh.h"
#include "Engine/Texture2D.h"
#include "FileHelpers.h"
#include "HAL/FileManager.h"
#include "Hash/Blake3.h"
#include "HangarApi.h"
#include "HangarMaterials.h"
#include "HangarSettings.h"
#include "HangarUi.h"
#include "InterchangeGenericAssetsPipeline.h"
#include "InterchangeGenericAssetsPipelineSharedSettings.h"
#include "InterchangeGenericMaterialPipeline.h"
#include "InterchangeGenericMeshPipeline.h"
#include "InterchangeGenericTexturePipeline.h"
#include "InterchangeGenericAnimationPipeline.h"
#include "InterchangeManager.h"
#include "Interfaces/IHttpResponse.h"
#include "Materials/MaterialInstanceConstant.h"
#include "Misc/FileHelper.h"
#include "Misc/Paths.h"
#include "Misc/ScopedSlowTask.h"
#include "ObjectTools.h"
#include "UObject/Package.h"
#include "UObject/StrongObjectPtr.h"

#define LOCTEXT_NAMESPACE "Hangar"

namespace
{
	constexpr int32 MaxParallelDownloads = 4;

	const TCHAR* ModelExtensions[] = {TEXT("fbx"), TEXT("obj"), TEXT("gltf"), TEXT("glb")};
	const TCHAR* ImageExtensions[] = {TEXT("png"), TEXT("jpg"), TEXT("jpeg"), TEXT("tga"), TEXT("tif"), TEXT("tiff"), TEXT("bmp"),
		TEXT("psd"), TEXT("exr"), TEXT("hdr")};

	bool HasExtension(const FString& Path, TArrayView<const TCHAR* const> Extensions)
	{
		const FString Ext = FPaths::GetExtension(Path).ToLower();
		return Algo::AnyOf(Extensions, [&Ext](const TCHAR* E) { return Ext == E; });
	}

	/** Hangar paths are normalised server-side; refuse anything that could leave the work dir anyway. */
	bool IsSafePath(const FString& Path)
	{
		return !Path.IsEmpty() && !Path.StartsWith(TEXT("/")) && !Path.Contains(TEXT("..")) && !Path.Contains(TEXT(":"))
			&& !Path.Contains(TEXT("\\"));
	}

	/** A content folder name from a slug or path segment. */
	FString FolderName(const FString& Name)
	{
		FString Out = ObjectTools::SanitizeObjectName(Name.TrimStartAndEnd());
		Out.ReplaceInline(TEXT(" "), TEXT("_"));
		return Out.IsEmpty() ? TEXT("item") : Out;
	}

	/** "Textures/Walls" → "/Textures/Walls" with every segment sanitised ("" stays ""). */
	FString FolderPath(const FString& Dir)
	{
		FString Out;
		TArray<FString> Parts;
		Dir.ParseIntoArray(Parts, TEXT("/"));
		for (const FString& Part : Parts)
		{
			Out += TEXT("/") + FolderName(Part);
		}
		return Out;
	}

	/** Lowercase letters and digits only: "Wall Brick.001" == "wall_brick_001". */
	FString SlotKey(const FString& Name)
	{
		FString Out;
		for (TCHAR C : Name.ToLower())
		{
			if (FChar::IsAlnum(C))
			{
				Out.AppendChar(C);
			}
		}
		return Out;
	}

	void SetMeta(UObject* Object, const TCHAR* Key, const FString& Value)
	{
		Object->GetPackage()->GetMetaData().SetValue(Object, Key, *Value);
	}

	FString GetMeta(UObject* Object, const TCHAR* Key)
	{
		const FString* Value = Object->GetPackage()->GetMetaData().FindValue(Object, Key);
		return Value ? *Value : FString();
	}

	void ApplyTextureKind(UTexture2D* Texture, HangarMaterials::ETextureKind Kind)
	{
		switch (Kind)
		{
		case HangarMaterials::ETextureKind::Color:
			Texture->CompressionSettings = TC_Default;
			Texture->SRGB = true;
			break;
		case HangarMaterials::ETextureKind::Linear:
			Texture->CompressionSettings = TC_Masks;
			Texture->SRGB = false;
			break;
		case HangarMaterials::ETextureKind::Normal:
			Texture->CompressionSettings = TC_Normalmap;
			Texture->SRGB = false;
			Texture->LODGroup = TEXTUREGROUP_WorldNormalMap;
			break;
		}
	}
}

FHangarImportJob::FHangarImportJob(const FHangarImport& InImport, const FString& InServer, const FString& InToken, TFunction<void()> InOnFinished)
	: Import_(InImport)
	, Server(InServer)
	, Token(InToken)
	, OnFinished(MoveTemp(InOnFinished))
{
}

void FHangarImportJob::Start()
{
	Note = HangarUi::Notify(FText::Format(LOCTEXT("Receiving", "Recebendo {0} v{1} de {2}…"), FText::FromString(Import_.AssetName),
		Import_.VersionNumber, FText::FromString(Import_.SentBy)), SNotificationItem::CS_Pending);
	Report(TEXT("downloading"), 0.f, FString());
	HangarApi::Get(Server / TEXT("api/versions") / Import_.VersionId / TEXT("bundle"), Token,
		[Self = AsShared()](int32 Code, const FString& Body) { Self->OnBundle(Code, Body); });
}

void FHangarImportJob::Report(const TCHAR* State, TOptional<float> Progress, const FString& Message, const TArray<FString>& Imported)
{
	TSharedRef<FJsonObject> Body = MakeShared<FJsonObject>();
	Body->SetStringField(TEXT("state"), State);
	if (Progress.IsSet())
	{
		Body->SetNumberField(TEXT("progress"), Progress.GetValue());
	}
	if (!Message.IsEmpty())
	{
		Body->SetStringField(TEXT("message"), Message);
	}
	TArray<TSharedPtr<FJsonValue>> Paths;
	for (const FString& Path : Imported)
	{
		Paths.Add(MakeShared<FJsonValueString>(Path));
	}
	Body->SetArrayField(TEXT("imported"), Paths);
	HangarApi::Post(Server / TEXT("api/bridge/deliveries") / Import_.DeliveryId, Token, Body,
		[](int32 Code, const FString& Text)
		{
			if (Code != 200)
			{
				UE_LOG(LogHangar, Warning, TEXT("Delivery report failed: %s"), *HangarApi::ErrorText(Code, Text));
			}
		});
}

FString FHangarImportJob::BlobPath(const FString& Hash) const
{
	return FPaths::ProjectSavedDir() / TEXT("Hangar/blobs") / Hash.Left(2) / Hash;
}

FString FHangarImportJob::WorkPath(const FString& Path) const
{
	return FPaths::ProjectSavedDir() / TEXT("Hangar") / FolderName(Import_.ProjectSlug) / FolderName(Import_.AssetSlug)
		/ FString::Printf(TEXT("v%d"), Import_.VersionNumber) / Path;
}

void FHangarImportJob::OnBundle(int32 Code, const FString& Body)
{
	if (Code != 200 || !FHangarBundle::Parse(Hangar::ParseObject(Body), Bundle))
	{
		Fail(FString::Printf(TEXT("não foi possível ler a versão: %s"), *HangarApi::ErrorText(Code, Body)));
		return;
	}
	if (!Bundle.bHasManifest)
	{
		Fail(TEXT("a versão ainda não foi processada pelo Hangar; tente de novo quando ela estiver pronta"));
		return;
	}

	// Only what the import uses: the model file and its material textures (or every image of a texture asset).
	const FHangarManifest& M = Bundle.Manifest;
	TSet<FString> Needed;
	if (HasExtension(M.PrimaryFile, ModelExtensions))
	{
		Needed.Add(M.PrimaryFile);
		for (const FHangarMaterial& Mat : M.Materials)
		{
			for (const FHangarMaterialTexture& Tex : Mat.Textures)
			{
				if (!Tex.Path.IsEmpty())
				{
					Needed.Add(Tex.Path);
				}
			}
		}
		for (const FHangarVariant& Variant : M.Variants)
		{
			for (const FHangarTextureOverride& Over : Variant.Overrides)
			{
				Needed.Add(Over.Path);
			}
		}
	}
	else if (M.Kind == TEXT("image") || Import_.Kind == TEXT("texture"))
	{
		for (const FHangarFile& File : Bundle.Files)
		{
			if (HasExtension(File.Path, ImageExtensions))
			{
				Needed.Add(File.Path);
			}
		}
	}
	else
	{
		// TODO: animation packages (AnimSequences on a chosen Skeleton) and audio.
		Fail(FString::Printf(TEXT("o plugin ainda não importa assets do tipo \"%s\""), M.Kind.IsEmpty() ? *Import_.Kind : *M.Kind));
		return;
	}

	for (const FHangarFile& File : Bundle.Files)
	{
		if (!Needed.Contains(File.Path))
		{
			continue;
		}
		if (!IsSafePath(File.Path) || File.Hash.Len() != 64)
		{
			Fail(FString::Printf(TEXT("caminho inválido no pacote: %s"), *File.Path));
			return;
		}
		FDownload& Download = Downloads.Add_GetRef({File, false});
		Download.bDone = IFileManager::Get().FileSize(*BlobPath(File.Hash)) == File.Size;
		if (!Download.bDone)
		{
			BytesTotal += File.Size;
		}
	}
	if (Downloads.IsEmpty())
	{
		Fail(TEXT("a versão não tem arquivos para importar"));
		return;
	}
	DownloadMore();
}

void FHangarImportJob::DownloadMore()
{
	if (bFailed)
	{
		return;
	}
	while (InFlight < MaxParallelDownloads && NextDownload < Downloads.Num())
	{
		const int32 Index = NextDownload++;
		if (Downloads[Index].bDone)
		{
			continue;
		}
		++InFlight;
		const FHangarFile File = Downloads[Index].File;
		TSharedRef<IHttpRequest, ESPMode::ThreadSafe> Request =
			HangarApi::NewRequest(TEXT("GET"), Server / TEXT("api/blobs") / File.Hash, Token);
		Request->OnProcessRequestComplete().BindLambda(
			[Self = AsShared(), Index, File](FHttpRequestPtr, FHttpResponsePtr Response, bool bConnected)
			{
				const int32 Code = bConnected && Response.IsValid() ? Response->GetResponseCode() : 0;
				if (Code != 200)
				{
					--Self->InFlight;
					Self->Fail(FString::Printf(TEXT("falha ao baixar %s: %s"), *File.Path,
						*HangarApi::ErrorText(Code, Response.IsValid() ? Response->GetContentAsString() : FString())));
					return;
				}
				TArray<uint8> Bytes = Response->GetContent();
				// Hash and write off the game thread; the cache is content-addressed.
				AsyncTask(ENamedThreads::AnyBackgroundThreadNormalTask, [Self, Index, File, Bytes = MoveTemp(Bytes)]() mutable
				{
					const FString Actual = LexToString(FBlake3::HashBuffer(Bytes.GetData(), Bytes.Num()));
					FString Error;
					if (Actual != File.Hash)
					{
						Error = FString::Printf(TEXT("%s chegou corrompido (hash diferente)"), *File.Path);
					}
					else if (!FFileHelper::SaveArrayToFile(Bytes, *Self->BlobPath(File.Hash)))
					{
						Error = FString::Printf(TEXT("não foi possível gravar %s"), *Self->BlobPath(File.Hash));
					}
					HangarApi::NextTick([Self, Index, Error, Size = Bytes.Num()]() mutable
					{
						--Self->InFlight;
						if (!Error.IsEmpty())
						{
							Self->Fail(Error);
							return;
						}
						Self->BytesDone += Size;
						Self->OnDownloaded(Index);
					});
				});
			});
		Request->ProcessRequest();
	}
	if (InFlight == 0 && NextDownload >= Downloads.Num())
	{
		// Never from inside a task or a callback that may be one (see HangarApi::NextTick).
		HangarApi::NextTick([Self = AsShared()] { Self->Import(); });
	}
}

void FHangarImportJob::OnDownloaded(int32 Index)
{
	if (bFailed)
	{
		return;
	}
	Downloads[Index].bDone = true;
	Report(TEXT("downloading"), BytesTotal > 0 ? float(double(BytesDone) / double(BytesTotal)) : 1.f, FString());
	DownloadMore();
}

void FHangarImportJob::Import()
{
	if (bFailed)
	{
		return;
	}
	// Lay the files out like the version tree (Saved/Hangar/<project>/<asset>/v<N>/…).
	for (const FDownload& Download : Downloads)
	{
		const FString Dest = WorkPath(Download.File.Path);
		if (IFileManager::Get().FileSize(*Dest) == Download.File.Size)
		{
			continue;
		}
		IFileManager::Get().MakeDirectory(*FPaths::GetPath(Dest), true);
		if (IFileManager::Get().Copy(*Dest, *BlobPath(Download.File.Hash)) != COPY_OK)
		{
			Fail(FString::Printf(TEXT("não foi possível copiar %s"), *Dest));
			return;
		}
	}
	Report(TEXT("importing"), {}, FString());

	const UHangarSettings* Settings = GetDefault<UHangarSettings>();
	const FHangarManifest& M = Bundle.Manifest;
	const FString Root = Settings->Root();
	const FString ProjectRoot = Root / FolderName(Import_.ProjectSlug);
	const FString AssetRoot = ProjectRoot / FolderName(Import_.AssetSlug);
	const FString AssetName = FolderName(Import_.AssetName);
	const bool bModel = HasExtension(M.PrimaryFile, ModelExtensions);
	TArray<FString> Warnings;
	TArray<UObject*> Imported;

	FScopedSlowTask Slow(4.f, FText::Format(LOCTEXT("Importing", "Importando {0} v{1}…"), FText::FromString(Import_.AssetName), Import_.VersionNumber));
	Slow.MakeDialog();

	FString Error;
	if (bModel && !HangarMaterials::EnsureMasters(Root, Error))
	{
		Fail(Error);
		return;
	}

	// Textures: kind from the first slot that uses each file.
	Slow.EnterProgressFrame(1.f, LOCTEXT("Textures", "Texturas"));
	TMap<FString, HangarMaterials::ETextureKind> Kinds;
	if (bModel)
	{
		for (const FHangarMaterial& Mat : M.Materials)
		{
			for (const FHangarMaterialTexture& Tex : Mat.Textures)
			{
				if (!Tex.Path.IsEmpty() && !Kinds.Contains(Tex.Path))
				{
					Kinds.Add(Tex.Path, HangarMaterials::KindForSlot(Tex.Slot));
				}
			}
		}
		for (const FHangarVariant& Variant : M.Variants)
		{
			for (const FHangarTextureOverride& Over : Variant.Overrides)
			{
				if (!Kinds.Contains(Over.Path))
				{
					Kinds.Add(Over.Path, HangarMaterials::KindForSlot(Over.Slot));
				}
			}
		}
	}
	else
	{
		for (const FDownload& Download : Downloads)
		{
			Kinds.Add(Download.File.Path, HangarMaterials::ETextureKind::Color);
		}
	}

	IAssetTools& AssetTools = FModuleManager::LoadModuleChecked<FAssetToolsModule>("AssetTools").Get();
	TMap<FString, UTexture2D*> Textures;
	for (const TPair<FString, HangarMaterials::ETextureKind>& Pair : Kinds)
	{
		const FString& Path = Pair.Key;
		const FDownload* Download = Downloads.FindByPredicate([&Path](const FDownload& D) { return D.File.Path == Path; });
		if (!Download)
		{
			Warnings.Add(FString::Printf(TEXT("textura %s não está no pacote da versão"), *Path));
			continue;
		}
		// "@<set>/dir/x.png" → <projeto>/TextureSets/<set>/dir, shared by every asset of the set.
		FString Folder;
		FString Rest = Path;
		if (Path.StartsWith(TEXT("@")))
		{
			FString Prefix;
			Path.Split(TEXT("/"), &Prefix, &Rest);
			Folder = ProjectRoot / TEXT("TextureSets") / FolderName(Prefix.Mid(1));
		}
		else
		{
			Folder = bModel ? AssetRoot / TEXT("Textures") : AssetRoot;
		}
		Folder += FolderPath(FPaths::GetPath(Rest));
		FString Name = FolderName(FPaths::GetBaseFilename(Rest));
		if (!Name.StartsWith(TEXT("T_")))
		{
			Name = TEXT("T_") + Name;
		}

		UTexture2D* Texture = LoadObject<UTexture2D>(nullptr, *FString::Printf(TEXT("%s/%s.%s"), *Folder, *Name, *Name), nullptr, LOAD_NoWarn | LOAD_Quiet);
		if (!Texture || GetMeta(Texture, TEXT("HangarHash")) != Download->File.Hash)
		{
			UAssetImportTask* Task = NewObject<UAssetImportTask>();
			Task->Filename = WorkPath(Path);
			Task->DestinationPath = Folder;
			Task->DestinationName = Name;
			Task->bAutomated = true;
			Task->bReplaceExisting = true;
			Task->bSave = false;
			AssetTools.ImportAssetTasks({Task});
			Texture = nullptr;
			for (UObject* Object : Task->GetObjects())
			{
				Texture = Texture ? Texture : Cast<UTexture2D>(Object);
			}
			if (!Texture)
			{
				Warnings.Add(FString::Printf(TEXT("não foi possível importar a textura %s"), *Path));
				continue;
			}
			SetMeta(Texture, TEXT("HangarHash"), Download->File.Hash);
			SetMeta(Texture, TEXT("HangarPath"), Path);
			SetMeta(Texture, TEXT("HangarServer"), Server);
		}
		ApplyTextureKind(Texture, Pair.Value);
		Texture->PostEditChange();
		Texture->MarkPackageDirty();
		Textures.Add(Path, Texture);
		Imported.Add(Texture);
	}

	// Mesh, through Interchange with its own materials and textures switched off.
	Slow.EnterProgressFrame(1.f, LOCTEXT("Mesh", "Malha"));
	UStaticMesh* StaticMesh = nullptr;
	USkeletalMesh* SkeletalMesh = nullptr;
	if (bModel)
	{
		TStrongObjectPtr<UInterchangeGenericAssetsPipeline> Pipeline(NewObject<UInterchangeGenericAssetsPipeline>(GetTransientPackage()));
		Pipeline->MaterialPipeline->bImportMaterials = false;
		Pipeline->MaterialPipeline->TexturePipeline->bImportTextures = false;
		// TODO: animations of the model file (AnimSequences on its skeleton).
		Pipeline->AnimationPipeline->bImportAnimations = false;
		Pipeline->CommonMeshesProperties->ForceAllMeshAsType =
			M.bHasSkin ? EInterchangeForceMeshType::IFMT_None : EInterchangeForceMeshType::IFMT_StaticMesh;
		Pipeline->MeshPipeline->CombineStaticMeshesBehavior = EInterchangeCombineStaticMeshesBehavior::All;
		// TODO: authored LODs from separate _LOD1… files (ImportStaticMeshLOD / ImportSkeletalMeshLOD).

		FImportAssetParameters Params;
		Params.bIsAutomated = true;
		Params.bReplaceExisting = true;
		Params.DestinationName = (M.bHasSkin ? TEXT("SK_") : TEXT("SM_")) + AssetName;
		Params.OverridePipelines.Add(FSoftObjectPath(Pipeline.Get()));
		UInterchangeSourceData* Source = UInterchangeManager::CreateSourceData(WorkPath(M.PrimaryFile));
		TArray<UObject*> Objects;
		const bool bOk = UInterchangeManager::GetInterchangeManager().ImportAsset(AssetRoot, Source, Params, Objects);
		for (UObject* Object : Objects)
		{
			StaticMesh = StaticMesh ? StaticMesh : Cast<UStaticMesh>(Object);
			SkeletalMesh = SkeletalMesh ? SkeletalMesh : Cast<USkeletalMesh>(Object);
			if (Object && (Object->IsA<UStaticMesh>() || Object->IsA<USkeletalMesh>() || Object->IsA<USkeleton>()))
			{
				Imported.Add(Object);
			}
		}
		if (!bOk || (!StaticMesh && !SkeletalMesh))
		{
			Fail(FString::Printf(TEXT("o Unreal não importou %s (veja o Output Log; o FBX via Interchange precisa de Interchange.FeatureFlags.Import.FBX)"),
				*M.PrimaryFile));
			return;
		}
	}

	// Material instances, assigned to the mesh slots by material name.
	Slow.EnterProgressFrame(1.f, LOCTEXT("Materials", "Materiais"));
	if (bModel)
	{
		const FString MaterialsFolder = AssetRoot / TEXT("Materials");
		TMap<FString, UMaterialInstanceConstant*> BySlot;
		for (const FHangarMaterial& Mat : M.Materials)
		{
			const FString Name = FString::Printf(TEXT("MI_%s_%s"), *AssetName, *FolderName(Mat.Name));
			if (UMaterialInstanceConstant* Instance = HangarMaterials::BuildInstance(Root, MaterialsFolder, Name, Mat, Textures, {}, Warnings))
			{
				BySlot.Add(SlotKey(Mat.Name), Instance);
				Imported.Add(Instance);
			}
		}
		// Skins: one more instance per material a variant changes (not assigned).
		for (const FHangarVariant& Variant : M.Variants)
		{
			if (Variant.bIsDefault)
			{
				continue;
			}
			for (const FHangarMaterial& Mat : M.Materials)
			{
				TMap<FString, FString> Overrides;
				for (const FHangarTextureOverride& Over : Variant.Overrides)
				{
					if (Over.Material.Equals(Mat.Name, ESearchCase::IgnoreCase))
					{
						Overrides.Add(Over.Slot, Over.Path);
					}
				}
				if (Overrides.IsEmpty())
				{
					continue;
				}
				const FString Name = FString::Printf(TEXT("MI_%s_%s_%s"), *AssetName, *FolderName(Mat.Name), *FolderName(Variant.Name));
				if (UMaterialInstanceConstant* Instance = HangarMaterials::BuildInstance(Root, MaterialsFolder / TEXT("Variants"), Name, Mat, Textures, Overrides, Warnings))
				{
					Imported.Add(Instance);
				}
			}
		}

		auto Assign = [&](FName SlotName, FName ImportedName, TObjectPtr<UMaterialInterface>& Target)
		{
			UMaterialInstanceConstant* const* Found = BySlot.Find(SlotKey(ImportedName.IsNone() ? SlotName.ToString() : ImportedName.ToString()));
			if (!Found)
			{
				Found = BySlot.Find(SlotKey(SlotName.ToString()));
			}
			if (Found)
			{
				Target = *Found;
			}
			else
			{
				Warnings.Add(FString::Printf(TEXT("slot de material \"%s\" sem material correspondente no Hangar"), *SlotName.ToString()));
			}
		};
		if (StaticMesh)
		{
			for (FStaticMaterial& Slot : StaticMesh->GetStaticMaterials())
			{
				Assign(Slot.MaterialSlotName, Slot.ImportedMaterialSlotName, Slot.MaterialInterface);
			}
			StaticMesh->PostEditChange();
			StaticMesh->MarkPackageDirty();
		}
		if (SkeletalMesh)
		{
			for (FSkeletalMaterial& Slot : SkeletalMesh->GetMaterials())
			{
				Assign(Slot.MaterialSlotName, Slot.ImportedMaterialSlotName, Slot.MaterialInterface);
			}
			SkeletalMesh->PostEditChange();
			SkeletalMesh->MarkPackageDirty();
		}

		UObject* Mesh = StaticMesh ? static_cast<UObject*>(StaticMesh) : SkeletalMesh;
		TArray<FString> Sets;
		for (const FHangarTextureSetUse& Set : M.TextureSets)
		{
			if (!Set.VersionId.IsEmpty())
			{
				Sets.Add(FString::Printf(TEXT("%s:%d"), *Set.AssetId, Set.VersionNumber));
			}
		}
		SetMeta(Mesh, TEXT("HangarServer"), Server);
		SetMeta(Mesh, TEXT("HangarAssetId"), Import_.AssetId);
		SetMeta(Mesh, TEXT("HangarVersionId"), Import_.VersionId);
		SetMeta(Mesh, TEXT("HangarVersion"), FString::FromInt(Import_.VersionNumber));
		SetMeta(Mesh, TEXT("HangarTextureSets"), FString::Join(Sets, TEXT(",")));
		Mesh->MarkPackageDirty();
	}

	Slow.EnterProgressFrame(1.f, LOCTEXT("Saving", "Salvando"));
	TArray<UPackage*> Packages;
	for (UObject* Object : Imported)
	{
		Packages.AddUnique(Object->GetPackage());
	}
	if (bModel)
	{
		// Masters and their default textures, when they were just created.
		const TCHAR* Shared[] = {TEXT("M_HangarOpaque"), TEXT("M_HangarMasked"), TEXT("M_HangarTranslucent"), TEXT("T_HangarWhite"),
			TEXT("T_HangarWhiteMask"), TEXT("T_HangarFlatNormal")};
		for (const TCHAR* Name : Shared)
		{
			if (UPackage* Package = FindPackage(nullptr, *(Root / TEXT("_Materials") / Name)))
			{
				Packages.AddUnique(Package);
			}
		}
	}
	UEditorLoadingAndSavingUtils::SavePackages(Packages, true);
	Finish(Imported, Warnings);
}

void FHangarImportJob::Fail(const FString& Message)
{
	if (bFailed)
	{
		return;
	}
	bFailed = true;
	UE_LOG(LogHangar, Error, TEXT("%s v%d: %s"), *Import_.AssetName, Import_.VersionNumber, *Message);
	Report(TEXT("failed"), {}, Message);
	HangarUi::Finish(Note, FText::Format(LOCTEXT("Failed", "Falha ao importar {0}: {1}"), FText::FromString(Import_.AssetName),
		FText::FromString(Message)), SNotificationItem::CS_Fail);
	// Downloads still in flight keep the job alive through their callbacks.
	Done();
}

void FHangarImportJob::Done()
{
	if (TFunction<void()> Callback = MoveTemp(OnFinished))
	{
		OnFinished = nullptr;
		Callback();
	}
}

void FHangarImportJob::Finish(const TArray<UObject*>& Imported, const TArray<FString>& Warnings)
{
	TArray<FString> Paths;
	for (const UObject* Object : Imported)
	{
		Paths.Add(Object->GetPathName());
	}
	for (const FString& Warning : Warnings)
	{
		UE_LOG(LogHangar, Warning, TEXT("%s: %s"), *Import_.AssetName, *Warning);
	}
	const FString Message = Warnings.IsEmpty()
		? FString::Printf(TEXT("%d asset(s) importado(s)"), Imported.Num())
		: FString::Printf(TEXT("%d asset(s) importado(s), %d aviso(s): %s"), Imported.Num(), Warnings.Num(), *FString::Join(Warnings, TEXT("; ")));
	Report(TEXT("done"), 1.f, Message, Paths);

	if (Note.IsValid())
	{
		Note->ExpireAndFadeout();
	}
	TArray<TWeakObjectPtr<UObject>> Weak(Imported);
	FNotificationButtonInfo Show(LOCTEXT("Show", "Mostrar no Content Browser"), FText::GetEmpty(),
		FSimpleDelegate::CreateLambda([Weak]
		{
			TArray<UObject*> Objects;
			for (const TWeakObjectPtr<UObject>& Object : Weak)
			{
				if (Object.IsValid())
				{
					Objects.Add(Object.Get());
				}
			}
			GEditor->SyncBrowserToObjects(Objects);
		}),
		SNotificationItem::CS_Success);
	const FText Text = Warnings.IsEmpty()
		? FText::Format(LOCTEXT("Done", "{0} v{1} importado"), FText::FromString(Import_.AssetName), Import_.VersionNumber)
		: FText::Format(LOCTEXT("DoneWarnings", "{0} v{1} importado com {2} aviso(s) (veja o Output Log)"),
			FText::FromString(Import_.AssetName), Import_.VersionNumber, Warnings.Num());
	HangarUi::Notify(Text, SNotificationItem::CS_Success, {Show});
	Done();
}

#undef LOCTEXT_NAMESPACE
