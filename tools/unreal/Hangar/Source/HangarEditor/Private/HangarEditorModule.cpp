#include "Editor.h"
#include "HangarBridgeSubsystem.h"
#include "ISettingsModule.h"
#include "Modules/ModuleManager.h"
#include "Styling/AppStyle.h"
#include "ToolMenus.h"

#define LOCTEXT_NAMESPACE "Hangar"

namespace
{
	UHangarBridgeSubsystem* Bridge()
	{
		return GEditor ? GEditor->GetEditorSubsystem<UHangarBridgeSubsystem>() : nullptr;
	}

	void FillMenu(UToolMenu* Menu)
	{
		FToolMenuSection& Section = Menu->AddSection(TEXT("Hangar"), LOCTEXT("Section", "Hangar"));
		const UHangarBridgeSubsystem* B = Bridge();
		const bool bPaired = B && B->HasToken();
		Section.AddMenuEntry(TEXT("Connect"),
			bPaired ? LOCTEXT("PairAgain", "Conectar de novo…") : LOCTEXT("Pair", "Conectar ao Hangar…"),
			LOCTEXT("PairTip", "Abre o Hangar no navegador para autorizar este editor"),
			FSlateIcon(FAppStyle::GetAppStyleSetName(), "Icons.Link"),
			FUIAction(FExecuteAction::CreateLambda([] { if (UHangarBridgeSubsystem* S = Bridge()) S->Pair(); })));
		if (bPaired && B->GetState() != EHangarState::Connected)
		{
			Section.AddMenuEntry(TEXT("Reconnect"), LOCTEXT("Reconnect", "Reconectar"), LOCTEXT("ReconnectTip", "Tenta conectar agora"),
				FSlateIcon(FAppStyle::GetAppStyleSetName(), "Icons.Refresh"),
				FUIAction(FExecuteAction::CreateLambda([] { if (UHangarBridgeSubsystem* S = Bridge()) S->Reconnect(); })));
		}
		Section.AddMenuEntry(TEXT("Disconnect"), LOCTEXT("Disconnect", "Desconectar"),
			LOCTEXT("DisconnectTip", "Esquece o token deste computador"), FSlateIcon(FAppStyle::GetAppStyleSetName(), "Icons.X"),
			FUIAction(FExecuteAction::CreateLambda([] { if (UHangarBridgeSubsystem* S = Bridge()) S->Disconnect(); }),
				FCanExecuteAction::CreateLambda([bPaired] { return bPaired; })));
		Section.AddSeparator(NAME_None);
		Section.AddMenuEntry(TEXT("Browser"), LOCTEXT("Browser", "Abrir Hangar no navegador"), FText::GetEmpty(),
			FSlateIcon(FAppStyle::GetAppStyleSetName(), "Icons.Server"),
			FUIAction(FExecuteAction::CreateLambda([] { if (const UHangarBridgeSubsystem* S = Bridge()) S->OpenInBrowser(); })));
		Section.AddMenuEntry(TEXT("Settings"), LOCTEXT("Settings", "Configurações"), LOCTEXT("SettingsTip", "Servidor e pasta de destino"),
			FSlateIcon(FAppStyle::GetAppStyleSetName(), "Icons.Settings"),
			FUIAction(FExecuteAction::CreateLambda([]
			{
				FModuleManager::LoadModuleChecked<ISettingsModule>("Settings").ShowViewer("Editor", "Plugins", "Hangar");
			})));
	}
}

class FHangarEditorModule : public IModuleInterface
{
public:
	virtual void StartupModule() override
	{
		UToolMenus::RegisterStartupCallback(FSimpleMulticastDelegate::FDelegate::CreateRaw(this, &FHangarEditorModule::RegisterMenus));
	}

	virtual void ShutdownModule() override
	{
		UToolMenus::UnRegisterStartupCallback(this);
		UToolMenus::UnregisterOwner(this);
	}

private:
	void RegisterMenus()
	{
		FToolMenuOwnerScoped Owner(this);
		UToolMenu* Toolbar = UToolMenus::Get()->ExtendMenu("LevelEditor.LevelEditorToolBar.User");
		FToolMenuSection& Section = Toolbar->FindOrAddSection("Hangar");
		FToolMenuEntry Entry = FToolMenuEntry::InitComboButton(TEXT("HangarCombo"), FUIAction(),
			FNewToolMenuChoice(FNewToolMenuDelegate::CreateStatic(&FillMenu)),
			TAttribute<FText>::CreateLambda([]
			{
				const UHangarBridgeSubsystem* B = Bridge();
				return B ? B->StatusText() : LOCTEXT("Label", "Hangar");
			}),
			LOCTEXT("Tip", "Recebe assets enviados pelo Hangar (\"Enviar para Unreal\")"),
			TAttribute<FSlateIcon>::CreateLambda([]
			{
				const UHangarBridgeSubsystem* B = Bridge();
				const bool bOnline = B && B->GetState() == EHangarState::Connected;
				return FSlateIcon(FAppStyle::GetAppStyleSetName(), bOnline ? "Icons.Link" : "Icons.Unlink");
			}));
		Entry.StyleNameOverride = "CalloutToolbar";
		Section.AddEntry(Entry);
	}
};

IMPLEMENT_MODULE(FHangarEditorModule, HangarEditor)

#undef LOCTEXT_NAMESPACE
