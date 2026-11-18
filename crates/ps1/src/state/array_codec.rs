use serde::{Deserialize, Serialize};

// Serde supports arrays up to 32 elements. Validate larger hardware tables
// through a length-checked temporary vector without unsafe initialization.
pub(crate) mod array {
    use super::{Deserialize, Serialize};

    pub fn serialize<T: Serialize, S: serde::Serializer, const N: usize>(
        value: &[T; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_slice().serialize(serializer)
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[T; N], D::Error> {
        Vec::<T>::deserialize(deserializer)?
            .try_into()
            .map_err(|_: Vec<T>| serde::de::Error::custom("Invalid hardware table length"))
    }
}

pub(crate) mod matrix {
    use super::{Deserialize, Serialize};

    pub fn serialize<T: Serialize, S: serde::Serializer, const N: usize, const M: usize>(
        value: &[[T; N]; M],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .map(<[T; N]>::as_slice)
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<
        'de,
        T: Deserialize<'de>,
        D: serde::Deserializer<'de>,
        const N: usize,
        const M: usize,
    >(
        deserializer: D,
    ) -> Result<[[T; N]; M], D::Error> {
        Vec::<Vec<T>>::deserialize(deserializer)?
            .into_iter()
            .map(|row| {
                row.try_into()
                    .map_err(|_: Vec<T>| serde::de::Error::custom("Invalid hardware table width"))
            })
            .collect::<Result<Vec<[T; N]>, D::Error>>()?
            .try_into()
            .map_err(|_: Vec<[T; N]>| serde::de::Error::custom("Invalid hardware table height"))
    }
}
