#pragma once

#include "CoreMinimal.h"
#include "Dom/JsonObject.h"
#include "Interfaces/IHttpRequest.h"

DECLARE_LOG_CATEGORY_EXTERN(LogHangar, Log, All);

namespace HangarApi
{
	/** Called on the game thread. Code 0 = no response (connection failed). */
	using FDone = TFunction<void(int32 Code, const FString& Body)>;

	/** Access token stored for a server (shared by every project of this user). */
	FString LoadToken(const FString& Server);
	void SaveToken(const FString& Server, const FString& Token);
	void ForgetToken(const FString& Server);

	/** "<computer>" and "<project> · UE <version>", sent when pairing and connecting. */
	FString DeviceName();
	FString DeviceDetail();

	TSharedRef<IHttpRequest, ESPMode::ThreadSafe> NewRequest(const FString& Verb, const FString& Url, const FString& Token);
	void Get(const FString& Url, const FString& Token, FDone Done);
	void Post(const FString& Url, const FString& Token, const TSharedRef<FJsonObject>& Body, FDone Done);

	/**
	 * Runs Fn on the game thread at the next core ticker tick (callable from any thread).
	 * Use it instead of AsyncTask(GameThread): that runs inside a task-graph task, and an
	 * Interchange import started from there waits on the task graph and asserts
	 * (RecursionGuard in TaskGraph.cpp).
	 */
	void NextTick(TFunction<void()> Fn);

	/** First line of an error body, for notifications. */
	FString ErrorText(int32 Code, const FString& Body);
}
