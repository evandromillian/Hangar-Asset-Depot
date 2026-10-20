#pragma once

#include "CoreMinimal.h"
#include "Engine/DeveloperSettings.h"
#include "HangarSettings.generated.h"

/** Editor Preferences > Plugins > Hangar (per user and project). */
UCLASS(config = EditorPerProjectUserSettings, meta = (DisplayName = "Hangar"))
class UHangarSettings : public UDeveloperSettings
{
	GENERATED_BODY()

public:
	/** Address of the Hangar server, e.g. https://hangar.example.com */
	UPROPERTY(config, EditAnywhere, Category = "Servidor", meta = (DisplayName = "Servidor"))
	FString ServerUrl = TEXT("http://localhost:8100");

	/** Content folder that receives the imported assets (<raiz>/<projeto>/<asset>). */
	UPROPERTY(config, EditAnywhere, Category = "Importação", meta = (DisplayName = "Pasta de destino", ContentDir))
	FDirectoryPath ImportRoot{TEXT("/Game/Hangar")};

	/** Connect to Hangar when the editor opens, if this machine is paired. */
	UPROPERTY(config, EditAnywhere, Category = "Servidor", meta = (DisplayName = "Conectar ao abrir o editor"))
	bool bAutoConnect = true;

	virtual FName GetContainerName() const override { return TEXT("Editor"); }
	virtual FName GetCategoryName() const override { return TEXT("Plugins"); }
	virtual FName GetSectionName() const override { return TEXT("Hangar"); }

	/** Server URL without a trailing slash. */
	FString Server() const;
	/** Long package path of the import root ("/Game/Hangar"). */
	FString Root() const;
};
