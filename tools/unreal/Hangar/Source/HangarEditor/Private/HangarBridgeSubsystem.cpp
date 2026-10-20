#include "HangarBridgeSubsystem.h"

#include "Async/Async.h"
#include "GenericPlatform/GenericPlatformHttp.h"
#include "HAL/PlatformProcess.h"
#include "HangarApi.h"
#include "HangarImporter.h"
#include "HangarSettings.h"
#include "HangarSse.h"
#include "HangarUi.h"
#include "Interfaces/IHttpResponse.h"
#include "Misc/App.h"

#include <atomic>

#define LOCTEXT_NAMESPACE "Hangar"

namespace
{
	/** The stream gets a ": ping" comment every 15 s; three missed means it's dead. */
	constexpr float StreamActivityTimeout = 45.f;
	constexpr float MaxBackoff = 30.f;
}

void UHangarBridgeSubsystem::Initialize(FSubsystemCollectionBase& Collection)
{
	Super::Initialize(Collection);
	UHangarSettings* Settings = GetMutableDefault<UHangarSettings>();
	Settings->OnSettingChanged().AddWeakLambda(this, [this](UObject*, FPropertyChangedEvent&)
	{
		if (GetDefault<UHangarSettings>()->Server() != Server && State != EHangarState::Pairing)
		{
			Reconnect();
		}
	});
	Server = Settings->Server();
	Token = HangarApi::LoadToken(Server);
	if (Settings->bAutoConnect && !Token.IsEmpty())
	{
		Connect();
	}
}

void UHangarBridgeSubsystem::Deinitialize()
{
	StopPairing();
	CloseStream();
	GetMutableDefault<UHangarSettings>()->OnSettingChanged().RemoveAll(this);
	Super::Deinitialize();
}

FText UHangarBridgeSubsystem::StatusText() const
{
	switch (State)
	{
	case EHangarState::Pairing:
		return LOCTEXT("StatePairing", "Hangar: aguardando aprovação…");
	case EHangarState::Connecting:
		return LOCTEXT("StateConnecting", "Hangar: conectando…");
	case EHangarState::Connected:
		return FText::Format(LOCTEXT("StateConnected", "Hangar: {0}"), FText::FromString(UserName));
	default:
		return LOCTEXT("StateDisconnected", "Hangar: desconectado");
	}
}

void UHangarBridgeSubsystem::SetState(EHangarState NewState)
{
	State = NewState;
}

// ------------------------------------------------------------------ stream

void UHangarBridgeSubsystem::Reconnect()
{
	Backoff = 1.f;
	Connect();
}

void UHangarBridgeSubsystem::Connect()
{
	CloseStream();
	Server = GetDefault<UHangarSettings>()->Server();
	Token = HangarApi::LoadToken(Server);
	if (Token.IsEmpty())
	{
		SetState(EHangarState::Disconnected);
		return;
	}
	SetState(EHangarState::Connecting);

	const int32 Gen = ++Generation;
	const FString Url = FString::Printf(TEXT("%s/api/bridge/events?app=unreal&name=%s&detail=%s"), *Server,
		*FGenericPlatformHttp::UrlEncode(HangarApi::DeviceName()), *FGenericPlatformHttp::UrlEncode(HangarApi::DeviceDetail()));
	Stream = HangarApi::NewRequest(TEXT("GET"), Url, Token);
	Stream->SetHeader(TEXT("Accept"), TEXT("text/event-stream"));
	Stream->SetTimeout(0.f); // no total timeout: the stream lives as long as the editor
	Stream->SetActivityTimeout(StreamActivityTimeout);

	// The body arrives on the HTTP thread; events are handed to the game thread.
	TWeakObjectPtr<UHangarBridgeSubsystem> Weak(this);
	TSharedRef<std::atomic<int32>> Status = MakeShared<std::atomic<int32>>(0);
	TSharedRef<FHangarSseParser> Parser = MakeShared<FHangarSseParser>([Weak, Gen](const FString& Event, const FString& Data)
	{
		HangarApi::NextTick([Weak, Gen, Event, Data]
		{
			UHangarBridgeSubsystem* Self = Weak.Get();
			if (Self && Self->Generation == Gen)
			{
				Self->OnStreamEvent(Event, Data);
			}
		});
	});
	Stream->OnStatusCodeReceived().BindLambda([Status](FHttpRequestPtr, int32 Code) { *Status = Code; });
	Stream->SetResponseBodyReceiveStreamDelegateV2(FHttpRequestStreamDelegateV2::CreateLambda(
		[Parser, Status](void* Ptr, int64& Length)
		{
			// Error bodies (401 text) are not events.
			const int32 Code = *Status;
			if (Code == 0 || Code == 200)
			{
				Parser->Feed(static_cast<const uint8*>(Ptr), Length);
			}
		}));
	Stream->OnProcessRequestComplete().BindLambda([Weak, Gen](FHttpRequestPtr, FHttpResponsePtr Response, bool)
	{
		UHangarBridgeSubsystem* Self = Weak.Get();
		if (Self && Self->Generation == Gen)
		{
			Self->OnStreamClosed(Response.IsValid() ? Response->GetResponseCode() : 0);
		}
	});
	Stream->ProcessRequest();
}

void UHangarBridgeSubsystem::CloseStream()
{
	++Generation;
	if (ReconnectTicker.IsValid())
	{
		FTSTicker::GetCoreTicker().RemoveTicker(ReconnectTicker);
		ReconnectTicker.Reset();
	}
	if (Stream.IsValid())
	{
		Stream->CancelRequest();
		Stream.Reset();
	}
}

void UHangarBridgeSubsystem::ScheduleReconnect()
{
	const float Delay = Backoff;
	Backoff = FMath::Min(Backoff * 2.f, MaxBackoff);
	ReconnectTicker = FTSTicker::GetCoreTicker().AddTicker(FTickerDelegate::CreateWeakLambda(this, [this](float)
	{
		ReconnectTicker.Reset();
		Connect();
		return false;
	}), Delay);
}

void UHangarBridgeSubsystem::OnStreamClosed(int32 Code)
{
	Stream.Reset();
	if (Code == 401)
	{
		Revoked();
		return;
	}
	if (State == EHangarState::Connected)
	{
		UE_LOG(LogHangar, Log, TEXT("Hangar stream closed (HTTP %d), reconnecting"), Code);
	}
	SetState(EHangarState::Connecting);
	ScheduleReconnect();
}

void UHangarBridgeSubsystem::Revoked()
{
	HangarApi::ForgetToken(Server);
	Token.Empty();
	SetState(EHangarState::Disconnected);
	HangarUi::Notify(LOCTEXT("Revoked", "Conexão com o Hangar revogada — conecte de novo"), SNotificationItem::CS_Fail);
}

void UHangarBridgeSubsystem::OnStreamEvent(const FString& Event, const FString& Data)
{
	const TSharedPtr<FJsonObject> Json = Hangar::ParseObject(Data);
	if (Event == TEXT("hello"))
	{
		const TSharedPtr<FJsonObject>* User = nullptr;
		if (Json.IsValid() && Json->TryGetObjectField(TEXT("user"), User))
		{
			(*User)->TryGetStringField(TEXT("display_name"), UserName);
		}
		Backoff = 1.f;
		SetState(EHangarState::Connected);
		UE_LOG(LogHangar, Log, TEXT("Connected to %s as %s"), *Server, *UserName);
	}
	else if (Event == TEXT("import"))
	{
		FHangarImport Import;
		if (FHangarImport::Parse(Json, Import))
		{
			Enqueue(Import);
		}
		else
		{
			UE_LOG(LogHangar, Warning, TEXT("Ignoring malformed import event"));
		}
	}
}

// ----------------------------------------------------------------- pairing

void UHangarBridgeSubsystem::Pair()
{
	StopPairing();
	CloseStream();
	Server = GetDefault<UHangarSettings>()->Server();
	SetState(EHangarState::Pairing);
	TSharedRef<FJsonObject> Body = MakeShared<FJsonObject>();
	Body->SetStringField(TEXT("client"), TEXT("unreal"));
	Body->SetStringField(TEXT("device_name"), HangarApi::DeviceName());
	Body->SetStringField(TEXT("detail"), HangarApi::DeviceDetail());
	TWeakObjectPtr<UHangarBridgeSubsystem> Weak(this);
	HangarApi::Post(Server / TEXT("api/devices/pair"), FString(), Body, [Weak](int32 Code, const FString& Text)
	{
		if (UHangarBridgeSubsystem* Self = Weak.Get())
		{
			Self->OnPairStarted(Code, Text);
		}
	});
}

void UHangarBridgeSubsystem::OnPairStarted(int32 Code, const FString& Body)
{
	const TSharedPtr<FJsonObject> Json = Code == 200 ? Hangar::ParseObject(Body) : nullptr;
	FString UserCode, VerifyPath;
	double ExpiresIn = 600, Interval = 3;
	if (!Json.IsValid() || !Json->TryGetStringField(TEXT("device_code"), DeviceCode) || !Json->TryGetStringField(TEXT("user_code"), UserCode)
		|| !Json->TryGetStringField(TEXT("verify_path"), VerifyPath))
	{
		DeviceCode.Empty();
		SetState(EHangarState::Disconnected);
		Connect();
		HangarUi::Notify(FText::Format(LOCTEXT("PairFailed", "Não foi possível falar com o Hangar em {0}: {1}"),
			FText::FromString(Server), FText::FromString(HangarApi::ErrorText(Code, Body))), SNotificationItem::CS_Fail);
		return;
	}
	Json->TryGetNumberField(TEXT("expires_in"), ExpiresIn);
	Json->TryGetNumberField(TEXT("interval"), Interval);

	FPlatformProcess::LaunchURL(*(Server + VerifyPath), nullptr, nullptr);
	TWeakObjectPtr<UHangarBridgeSubsystem> Weak(this);
	FNotificationButtonInfo Cancel(LOCTEXT("Cancel", "Cancelar"), FText::GetEmpty(), FSimpleDelegate::CreateLambda([Weak]
	{
		if (UHangarBridgeSubsystem* Self = Weak.Get())
		{
			HangarUi::Finish(Self->PairNote, LOCTEXT("PairCancelled", "Conexão cancelada"), SNotificationItem::CS_None);
			Self->StopPairing();
			Self->Reconnect();
		}
	}), SNotificationItem::CS_Pending);
	PairNote = HangarUi::Notify(FText::Format(LOCTEXT("PairConfirm", "Confirme o código {0} no navegador"), FText::FromString(UserCode)),
		SNotificationItem::CS_Pending, {Cancel});
	PairDeadline = FPlatformTime::Seconds() + ExpiresIn;
	PairTicker = FTSTicker::GetCoreTicker().AddTicker(FTickerDelegate::CreateUObject(this, &UHangarBridgeSubsystem::PollPairing),
		float(FMath::Max(1.0, Interval)));
}

bool UHangarBridgeSubsystem::PollPairing(float)
{
	if (bPollInFlight)
	{
		return true;
	}
	if (FPlatformTime::Seconds() > PairDeadline)
	{
		HangarUi::Finish(PairNote, LOCTEXT("PairExpired", "O código expirou — conecte de novo"), SNotificationItem::CS_Fail);
		PairTicker.Reset();
		StopPairing();
		Reconnect();
		return false;
	}
	bPollInFlight = true;
	TSharedRef<FJsonObject> Body = MakeShared<FJsonObject>();
	Body->SetStringField(TEXT("device_code"), DeviceCode);
	TWeakObjectPtr<UHangarBridgeSubsystem> Weak(this);
	HangarApi::Post(Server / TEXT("api/devices/pair/poll"), FString(), Body, [Weak](int32 Code, const FString& Text)
	{
		UHangarBridgeSubsystem* Self = Weak.Get();
		if (!Self || Self->DeviceCode.IsEmpty())
		{
			return;
		}
		Self->bPollInFlight = false;
		const TSharedPtr<FJsonObject> Json = Code == 200 ? Hangar::ParseObject(Text) : nullptr;
		FString Status, NewToken;
		if (!Json.IsValid() || !Json->TryGetStringField(TEXT("status"), Status) || Status == TEXT("pending"))
		{
			return; // keep polling (a failed poll is retried until the code expires)
		}
		if (Status == TEXT("approved") && Json->TryGetStringField(TEXT("token"), NewToken))
		{
			FString Name;
			const TSharedPtr<FJsonObject>* User = nullptr;
			if (Json->TryGetObjectField(TEXT("user"), User))
			{
				(*User)->TryGetStringField(TEXT("display_name"), Name);
			}
			HangarApi::SaveToken(Self->Server, NewToken);
			HangarUi::Finish(Self->PairNote, FText::Format(LOCTEXT("Paired", "Unreal conectado ao Hangar como {0}"), FText::FromString(Name)),
				SNotificationItem::CS_Success);
			Self->StopPairing();
			Self->Reconnect();
			return;
		}
		HangarUi::Finish(Self->PairNote, Status == TEXT("denied") ? LOCTEXT("PairDenied", "Conexão recusada no navegador")
			: LOCTEXT("PairExpired2", "O código expirou — conecte de novo"), SNotificationItem::CS_Fail);
		Self->StopPairing();
		Self->Reconnect();
	});
	return true;
}

void UHangarBridgeSubsystem::StopPairing()
{
	if (PairTicker.IsValid())
	{
		FTSTicker::GetCoreTicker().RemoveTicker(PairTicker);
		PairTicker.Reset();
	}
	DeviceCode.Empty();
	bPollInFlight = false;
	PairNote.Reset();
	if (State == EHangarState::Pairing)
	{
		SetState(EHangarState::Disconnected);
	}
}

// ----------------------------------------------------------------- actions

void UHangarBridgeSubsystem::Disconnect()
{
	StopPairing();
	CloseStream();
	// Revoke it server-side as well (fire and forget: offline, it is still forgotten here
	// and can be revoked under Dispositivos).
	if (!Token.IsEmpty())
	{
		HangarApi::NewRequest(TEXT("DELETE"), Server + TEXT("/api/tokens/current"), Token)->ProcessRequest();
	}
	HangarApi::ForgetToken(Server);
	Token.Empty();
	UserName.Empty();
	SetState(EHangarState::Disconnected);
	HangarUi::Notify(LOCTEXT("Disconnected", "Desconectado do Hangar. O acesso deste editor foi revogado."),
		SNotificationItem::CS_None);
}

void UHangarBridgeSubsystem::OpenInBrowser() const
{
	FPlatformProcess::LaunchURL(*GetDefault<UHangarSettings>()->Server(), nullptr, nullptr);
}

// ------------------------------------------------------------------ imports

void UHangarBridgeSubsystem::Enqueue(const FHangarImport& Import)
{
	Queue.Add(Import);
	if (!Current.IsValid())
	{
		RunNext();
	}
}

void UHangarBridgeSubsystem::RunNext()
{
	if (Current.IsValid() || Queue.IsEmpty())
	{
		return;
	}
	const FHangarImport Import = Queue[0];
	Queue.RemoveAt(0);
	TWeakObjectPtr<UHangarBridgeSubsystem> Weak(this);
	Current = MakeShared<FHangarImportJob>(Import, Server, Token, [Weak]
	{
		// Next tick: the finished job may still be on the stack.
		HangarApi::NextTick([Weak]
		{
			if (UHangarBridgeSubsystem* Self = Weak.Get())
			{
				Self->Current.Reset();
				Self->RunNext();
			}
		});
	});
	Current->Start();
}

#undef LOCTEXT_NAMESPACE
