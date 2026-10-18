use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

macro_rules! string_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(&self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
        }

        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s { $($s => Ok($name::$variant),)+ other => Err(format!("invalid {}: {other}", stringify!($name))) }
            }
        }
    };
}

string_enum! {
    /// What an asset fundamentally is. Drives which preview the UI shows.
    AssetKind {
        Model => "model",
        Texture => "texture",
        Material => "material",
        Audio => "audio",
        Animation => "animation",
        Vfx => "vfx",
        Other => "other",
    }
}

string_enum! {
    /// Role of a user inside a project.
    Role {
        Owner => "owner",
        Editor => "editor",
        Viewer => "viewer",
    }
}

impl Role {
    pub fn can_write(&self) -> bool {
        matches!(self, Role::Owner | Role::Editor)
    }
    pub fn can_admin(&self) -> bool {
        matches!(self, Role::Owner)
    }
}

string_enum! {
    /// Processing state of a version (conversion, LODs, thumbnails).
    VersionStatus {
        Pending => "pending",
        Processing => "processing",
        Ready => "ready",
        Failed => "failed",
    }
}

string_enum! {
    /// Where a level of detail came from.
    LodSource {
        Authored => "authored",
        Generated => "generated",
    }
}

string_enum! {
    /// Semantic slot of a texture inside a PBR material.
    TextureSlot {
        BaseColor => "base_color",
        Normal => "normal",
        MetallicRoughness => "metallic_roughness",
        Orm => "orm",
        Roughness => "roughness",
        Metallic => "metallic",
        Occlusion => "occlusion",
        Emissive => "emissive",
        Height => "height",
        Opacity => "opacity",
        Specular => "specular",
        Unknown => "unknown",
    }
}

impl TextureSlot {
    /// Colour textures are sampled in sRGB, everything else is linear data.
    pub fn is_srgb(&self) -> bool {
        matches!(self, TextureSlot::BaseColor | TextureSlot::Emissive | TextureSlot::Specular)
    }

    pub fn label(&self) -> &'static str {
        match self {
            TextureSlot::BaseColor => "Base color",
            TextureSlot::Normal => "Normal",
            TextureSlot::MetallicRoughness => "Metal/Rough",
            TextureSlot::Orm => "ORM",
            TextureSlot::Roughness => "Roughness",
            TextureSlot::Metallic => "Metallic",
            TextureSlot::Occlusion => "AO",
            TextureSlot::Emissive => "Emissive",
            TextureSlot::Height => "Height",
            TextureSlot::Opacity => "Opacity",
            TextureSlot::Specular => "Specular",
            TextureSlot::Unknown => "?",
        }
    }
}

string_enum! {
    /// What kind of preview the pipeline produced for a version.
    PreviewKind {
        Model => "model",
        // Animation files without a mesh, previewed on reference skeletons.
        Animation => "animation",
        Image => "image",
        Audio => "audio",
        None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for k in AssetKind::ALL {
            assert_eq!(k.as_str().parse::<AssetKind>().unwrap(), *k);
            let json = serde_json::to_string(k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
        assert!("nope".parse::<Role>().is_err());
    }
}
