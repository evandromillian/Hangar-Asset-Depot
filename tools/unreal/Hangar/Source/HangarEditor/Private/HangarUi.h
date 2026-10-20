#pragma once

#include "CoreMinimal.h"
#include "Widgets/Notifications/SNotificationList.h"

namespace HangarUi
{
	/** Shows a notification. Pending ones stay until Finish is called. */
	TSharedPtr<SNotificationItem> Notify(const FText& Text, SNotificationItem::ECompletionState State,
		TArray<FNotificationButtonInfo> Buttons = {});

	/** Turns a pending notification into a final one (success or fail) that fades out. */
	void Finish(const TSharedPtr<SNotificationItem>& Item, const FText& Text, SNotificationItem::ECompletionState State);
}
