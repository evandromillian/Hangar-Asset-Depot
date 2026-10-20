#include "HangarApi.h"

#include "HangarTypes.h"
#include "Containers/Ticker.h"
#include "HttpModule.h"
#include "Interfaces/IHttpResponse.h"
#include "Misc/App.h"
#include "Misc/EngineVersion.h"
#include "Misc/FileHelper.h"
#include "Misc/Paths.h"
#include "HAL/PlatformProcess.h"

DEFINE_LOG_CATEGORY(LogHangar);

namespace
{
	/** <user settings>/Hangar/credentials.json: {"servers": {"<url>": {"token": "hgr_…"}}} */
	FString CredentialsFile()
	{
		return FPaths::Combine(FPlatformProcess::UserSettingsDir(), TEXT("Hangar"), TEXT("credentials.json"));
	}

	TSharedRef<FJsonObject> LoadCredentials()
	{
		FString Text;
		if (FFileHelper::LoadFileToString(Text, *CredentialsFile()))
		{
			if (TSharedPtr<FJsonObject> Json = Hangar::ParseObject(Text))
			{
				return Json.ToSharedRef();
			}
		}
		return MakeShared<FJsonObject>();
	}

	TSharedRef<FJsonObject> Servers(const TSharedRef<FJsonObject>& Credentials)
	{
		const TSharedPtr<FJsonObject>* Existing = nullptr;
		if (Credentials->TryGetObjectField(TEXT("servers"), Existing))
		{
			return Existing->ToSharedRef();
		}
		TSharedRef<FJsonObject> Created = MakeShared<FJsonObject>();
		Credentials->SetObjectField(TEXT("servers"), Created);
		return Created;
	}

	void StoreCredentials(const TSharedRef<FJsonObject>& Credentials)
	{
		if (!FFileHelper::SaveStringToFile(Hangar::ToJson(Credentials), *CredentialsFile()))
		{
			UE_LOG(LogHangar, Error, TEXT("Could not write %s"), *CredentialsFile());
		}
	}

	void Send(const TSharedRef<IHttpRequest, ESPMode::ThreadSafe>& Request, HangarApi::FDone Done)
	{
		Request->OnProcessRequestComplete().BindLambda(
			[Done = MoveTemp(Done)](FHttpRequestPtr, FHttpResponsePtr Response, bool bConnected)
			{
				if (Done)
				{
					Done(bConnected && Response.IsValid() ? Response->GetResponseCode() : 0,
						Response.IsValid() ? Response->GetContentAsString() : FString());
				}
			});
		Request->ProcessRequest();
	}
}

FString HangarApi::LoadToken(const FString& Server)
{
	const TSharedRef<FJsonObject> Credentials = LoadCredentials();
	const TSharedPtr<FJsonObject>* Entry = nullptr;
	FString Token;
	if (Servers(Credentials)->TryGetObjectField(Server, Entry))
	{
		(*Entry)->TryGetStringField(TEXT("token"), Token);
	}
	return Token;
}

void HangarApi::SaveToken(const FString& Server, const FString& Token)
{
	const TSharedRef<FJsonObject> Credentials = LoadCredentials();
	const TSharedRef<FJsonObject> Entry = MakeShared<FJsonObject>();
	Entry->SetStringField(TEXT("token"), Token);
	Servers(Credentials)->SetObjectField(Server, Entry);
	StoreCredentials(Credentials);
}

void HangarApi::ForgetToken(const FString& Server)
{
	const TSharedRef<FJsonObject> Credentials = LoadCredentials();
	Servers(Credentials)->RemoveField(Server);
	StoreCredentials(Credentials);
}

FString HangarApi::DeviceName()
{
	return FPlatformProcess::ComputerName();
}

void HangarApi::NextTick(TFunction<void()> Fn)
{
	FTSTicker::GetCoreTicker().AddTicker(FTickerDelegate::CreateLambda([Fn = MoveTemp(Fn)](float)
	{
		Fn();
		return false; // once
	}));
}

FString HangarApi::DeviceDetail()
{
	return FString::Printf(TEXT("%s · UE %s"), FApp::GetProjectName(), *FEngineVersion::Current().ToString(EVersionComponent::Patch));
}

TSharedRef<IHttpRequest, ESPMode::ThreadSafe> HangarApi::NewRequest(const FString& Verb, const FString& Url, const FString& Token)
{
	TSharedRef<IHttpRequest, ESPMode::ThreadSafe> Request = FHttpModule::Get().CreateRequest();
	Request->SetVerb(Verb);
	Request->SetURL(Url);
	if (!Token.IsEmpty())
	{
		Request->SetHeader(TEXT("Authorization"), TEXT("Bearer ") + Token);
	}
	return Request;
}

void HangarApi::Get(const FString& Url, const FString& Token, FDone Done)
{
	TSharedRef<IHttpRequest, ESPMode::ThreadSafe> Request = NewRequest(TEXT("GET"), Url, Token);
	Request->SetHeader(TEXT("Accept"), TEXT("application/json"));
	Send(Request, MoveTemp(Done));
}

void HangarApi::Post(const FString& Url, const FString& Token, const TSharedRef<FJsonObject>& Body, FDone Done)
{
	TSharedRef<IHttpRequest, ESPMode::ThreadSafe> Request = NewRequest(TEXT("POST"), Url, Token);
	Request->SetHeader(TEXT("Content-Type"), TEXT("application/json"));
	Request->SetContentAsString(Hangar::ToJson(Body));
	Send(Request, MoveTemp(Done));
}

FString HangarApi::ErrorText(int32 Code, const FString& Body)
{
	if (Code == 0)
	{
		return TEXT("sem resposta do servidor");
	}
	FString Line;
	if (!Body.Split(TEXT("\n"), &Line, nullptr))
	{
		Line = Body;
	}
	Line = Line.TrimStartAndEnd().Left(300);
	return Line.IsEmpty() ? FString::Printf(TEXT("HTTP %d"), Code) : FString::Printf(TEXT("%s (HTTP %d)"), *Line, Code);
}
