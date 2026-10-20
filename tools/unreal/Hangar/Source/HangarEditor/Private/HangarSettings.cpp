#include "HangarSettings.h"

FString UHangarSettings::Server() const
{
	FString Url = ServerUrl.TrimStartAndEnd();
	while (Url.EndsWith(TEXT("/")))
	{
		Url.LeftChopInline(1);
	}
	return Url;
}

FString UHangarSettings::Root() const
{
	FString Path = ImportRoot.Path.TrimStartAndEnd().Replace(TEXT("\\"), TEXT("/"));
	while (Path.EndsWith(TEXT("/")))
	{
		Path.LeftChopInline(1);
	}
	if (Path.IsEmpty())
	{
		return TEXT("/Game/Hangar");
	}
	return Path.StartsWith(TEXT("/")) ? Path : TEXT("/Game/") + Path;
}
