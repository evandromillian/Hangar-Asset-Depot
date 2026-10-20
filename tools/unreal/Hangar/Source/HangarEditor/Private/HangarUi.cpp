#include "HangarUi.h"

#include "Framework/Notifications/NotificationManager.h"

TSharedPtr<SNotificationItem> HangarUi::Notify(const FText& Text, SNotificationItem::ECompletionState State,
	TArray<FNotificationButtonInfo> Buttons)
{
	FNotificationInfo Info(Text);
	const bool bPending = State == SNotificationItem::CS_Pending;
	Info.bFireAndForget = !bPending;
	Info.ExpireDuration = Buttons.IsEmpty() ? 6.f : 12.f;
	Info.bUseThrobber = bPending;
	Info.bUseSuccessFailIcons = true;
	Info.ButtonDetails = MoveTemp(Buttons);
	TSharedPtr<SNotificationItem> Item = FSlateNotificationManager::Get().AddNotification(Info);
	if (Item.IsValid())
	{
		Item->SetCompletionState(State);
	}
	return Item;
}

void HangarUi::Finish(const TSharedPtr<SNotificationItem>& Item, const FText& Text, SNotificationItem::ECompletionState State)
{
	if (!Item.IsValid())
	{
		Notify(Text, State);
		return;
	}
	Item->SetText(Text);
	Item->SetCompletionState(State);
	Item->SetExpireDuration(6.f);
	Item->ExpireAndFadeout();
}
