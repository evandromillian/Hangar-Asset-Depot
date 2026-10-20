using UnrealBuildTool;

public class HangarEditor : ModuleRules
{
	public HangarEditor(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;

		PrivateDependencyModuleNames.AddRange(new string[]
		{
			"Core",
			"CoreUObject",
			"Engine",
			"Slate",
			"SlateCore",
			"ApplicationCore",
			"UnrealEd",
			"EditorSubsystem",
			"DeveloperSettings",
			"Settings",
			"ToolMenus",
			"HTTP",
			"Json",
			"Projects",
			"AssetTools",
			"AssetRegistry",
			"ContentBrowser",
			"MaterialEditor",
			"InterchangeCore",
			"InterchangeEngine",
			"InterchangePipelines",
		});
	}
}
