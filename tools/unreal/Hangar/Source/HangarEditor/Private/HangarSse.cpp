#include "HangarSse.h"

#include "Containers/StringConv.h"

void FHangarSseParser::Feed(const uint8* Bytes, int64 Length)
{
	Pending.Append(Bytes, Length);
	int32 Start = 0;
	for (int32 i = 0; i < Pending.Num(); ++i)
	{
		if (Pending[i] != '\n')
		{
			continue;
		}
		int32 End = i;
		if (End > Start && Pending[End - 1] == '\r')
		{
			--End;
		}
		// Lines end on '\n', so a line never splits a UTF-8 sequence.
		const FUTF8ToTCHAR Converted(reinterpret_cast<const ANSICHAR*>(Pending.GetData() + Start), End - Start);
		Line(FString(Converted.Length(), Converted.Get()));
		Start = i + 1;
	}
	Pending.RemoveAt(0, Start, EAllowShrinking::No);
}

void FHangarSseParser::Line(const FString& Text)
{
	if (Text.IsEmpty())
	{
		if (bHasData)
		{
			OnEvent(EventName.IsEmpty() ? TEXT("message") : EventName, Data);
		}
		EventName.Reset();
		Data.Reset();
		bHasData = false;
		return;
	}
	if (Text.StartsWith(TEXT(":")))
	{
		return;
	}
	FString Field = Text;
	FString Value;
	int32 Colon = INDEX_NONE;
	if (Text.FindChar(TEXT(':'), Colon))
	{
		Field = Text.Left(Colon);
		Value = Text.Mid(Colon + 1);
		if (Value.StartsWith(TEXT(" ")))
		{
			Value.RightChopInline(1);
		}
	}
	if (Field == TEXT("event"))
	{
		EventName = Value;
	}
	else if (Field == TEXT("data"))
	{
		if (bHasData)
		{
			Data += TEXT("\n");
		}
		Data += Value;
		bHasData = true;
	}
}
