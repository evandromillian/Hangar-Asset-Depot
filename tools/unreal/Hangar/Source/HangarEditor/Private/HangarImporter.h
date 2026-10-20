#pragma once

#include "CoreMinimal.h"
#include "HangarTypes.h"

class SNotificationItem;

/**
 * One "Enviar para Unreal" delivery: downloads the version's bundle (verified
 * by BLAKE3, cached by hash under Saved/Hangar/blobs), imports textures, mesh
 * and materials, and reports each step to /api/bridge/deliveries/{id}.
 * Network work is asynchronous; the import itself runs on the game thread.
 */
class FHangarImportJob : public TSharedFromThis<FHangarImportJob>
{
public:
	FHangarImportJob(const FHangarImport& InImport, const FString& InServer, const FString& InToken, TFunction<void()> InOnFinished);

	void Start();

private:
	struct FDownload
	{
		FHangarFile File;
		bool bDone = false;
	};

	void Report(const TCHAR* State, TOptional<float> Progress, const FString& Message, const TArray<FString>& Imported = {});
	void OnBundle(int32 Code, const FString& Body);
	void DownloadMore();
	void OnDownloaded(int32 Index);
	void Import();
	void Fail(const FString& Message);
	void Finish(const TArray<UObject*>& Imported, const TArray<FString>& Warnings);
	/** Tells the subsystem this job is over (once). */
	void Done();

	FString BlobPath(const FString& Hash) const;
	FString WorkPath(const FString& Path) const;

	FHangarImport Import_;
	FString Server;
	FString Token;
	TFunction<void()> OnFinished;
	TSharedPtr<SNotificationItem> Note;

	FHangarBundle Bundle;
	TArray<FDownload> Downloads;
	int32 NextDownload = 0;
	int32 InFlight = 0;
	int64 BytesTotal = 0;
	int64 BytesDone = 0;
	bool bFailed = false;
};
