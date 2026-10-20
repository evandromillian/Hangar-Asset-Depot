#pragma once

#include "CoreMinimal.h"
#include "Containers/Ticker.h"
#include "EditorSubsystem.h"
#include "HangarTypes.h"
#include "Interfaces/IHttpRequest.h"
#include "HangarBridgeSubsystem.generated.h"

class FHangarImportJob;
class SNotificationItem;

enum class EHangarState : uint8
{
	Disconnected,
	Pairing,
	Connecting,
	Connected,
};

/**
 * Keeps this editor reachable from Hangar's "Enviar para Unreal": holds
 * GET /api/bridge/events open (server-sent events, reconnecting with back-off)
 * and runs the imports it receives one at a time. Also pairs the machine
 * (device flow) to get the access token.
 */
UCLASS()
class UHangarBridgeSubsystem : public UEditorSubsystem
{
	GENERATED_BODY()

public:
	virtual void Initialize(FSubsystemCollectionBase& Collection) override;
	virtual void Deinitialize() override;

	/** "Conectar ao Hangar…": pairs this machine (browser approval), then connects. */
	void Pair();
	/** Closes the stream and forgets this machine's token for the server. */
	void Disconnect();
	/** Reconnects with the current settings (e.g. after changing the server). */
	void Reconnect();
	void OpenInBrowser() const;

	EHangarState GetState() const { return State; }
	bool HasToken() const { return !Token.IsEmpty(); }
	FText StatusText() const;

private:
	void Connect();
	void CloseStream();
	void ScheduleReconnect();
	void SetState(EHangarState NewState);
	void OnStreamEvent(const FString& Event, const FString& Data);
	void OnStreamClosed(int32 Code);
	void Revoked();

	void OnPairStarted(int32 Code, const FString& Body);
	bool PollPairing(float);
	void StopPairing();

	void Enqueue(const FHangarImport& Import);
	void RunNext();

	FString Server;
	FString Token;
	FString UserName;
	EHangarState State = EHangarState::Disconnected;

	/** Bumped whenever the stream is replaced, so callbacks of an old one are ignored. */
	int32 Generation = 0;
	TSharedPtr<IHttpRequest, ESPMode::ThreadSafe> Stream;
	FTSTicker::FDelegateHandle ReconnectTicker;
	float Backoff = 1.f;

	FString DeviceCode;
	double PairDeadline = 0;
	bool bPollInFlight = false;
	FTSTicker::FDelegateHandle PairTicker;
	TSharedPtr<SNotificationItem> PairNote;

	TArray<FHangarImport> Queue;
	TSharedPtr<FHangarImportJob> Current;
};
