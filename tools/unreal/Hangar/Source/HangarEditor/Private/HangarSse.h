#pragma once

#include "CoreMinimal.h"

/**
 * Incremental parser for a text/event-stream body. Feed it the bytes as they
 * arrive (chunks can split lines and UTF-8 sequences anywhere); it calls
 * OnEvent(name, data) for each complete event. Comments (": ping") are skipped.
 * Not thread-safe: one producer.
 */
class FHangarSseParser
{
public:
	using FOnEvent = TFunction<void(const FString& Event, const FString& Data)>;

	explicit FHangarSseParser(FOnEvent InOnEvent) : OnEvent(MoveTemp(InOnEvent)) {}

	void Feed(const uint8* Bytes, int64 Length);

private:
	void Line(const FString& Text);

	FOnEvent OnEvent;
	TArray<uint8> Pending;
	FString EventName;
	FString Data;
	bool bHasData = false;
};
