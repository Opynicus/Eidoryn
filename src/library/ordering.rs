use super::{IndexResult, Library};
use rusqlite::TransactionBehavior;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placement {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameMove {
    pub source: i64,
    pub target: i64,
    pub placement: Placement,
    /// Visible IDs in displayed order when dragging from an automatic sort.
    /// An empty list applies the move directly to the current manual order.
    pub initial_order: Vec<i64>,
}

impl GameMove {
    /// Apply the drop to unique game IDs in manual order. A missing endpoint
    /// leaves the order intact and returns false.
    pub fn apply(&self, ordered_ids: &mut [i64]) -> bool {
        if self.source == self.target {
            return ordered_ids.contains(&self.source);
        }
        if self.initial_order.is_empty() {
            return self.place(ordered_ids);
        }
        let mut indices: HashMap<_, _> = ordered_ids
            .iter()
            .enumerate()
            .map(|(index, &id)| (id, index))
            .collect();
        let mut visible = Vec::new();
        let mut slots = Vec::new();
        for &id in &self.initial_order {
            if let Some(index) = indices.remove(&id) {
                slots.push(index);
                visible.push(id);
            }
        }
        if !self.place(&mut visible) {
            return false;
        }
        slots.sort_unstable();
        for (index, id) in slots.into_iter().zip(visible) {
            ordered_ids[index] = id;
        }
        true
    }

    fn place(&self, ids: &mut [i64]) -> bool {
        let Some(source) = ids.iter().position(|&id| id == self.source) else {
            return false;
        };
        let Some(target) = ids.iter().position(|&id| id == self.target) else {
            return false;
        };
        if source < target {
            let end = target + usize::from(self.placement == Placement::After);
            ids[source..end].rotate_left(1);
        } else {
            let start = target + usize::from(self.placement == Placement::After);
            ids[start..=source].rotate_right(1);
        }
        true
    }
}

impl Library {
    /// Commit a drop and return every game ID in its new zero-based position.
    pub fn move_game(&self, movement: &GameMove) -> Result<Vec<i64>, String> {
        self.persist_game_move(movement).map_err(|e| e.to_string())
    }

    fn persist_game_move(&self, movement: &GameMove) -> IndexResult<Vec<i64>> {
        let mut db = self.connection()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ordered = Vec::new();
        let mut visible = Vec::new();
        let mut slots = Vec::new();
        {
            let mut query = tx.prepare(
                "SELECT g.id, NOT g.hidden AND g.id IN (
                     SELECT d.game_id FROM listed_locations l JOIN discs d ON d.id = l.disc_id
                 ) FROM games g
                 ORDER BY g.manual_position IS NULL, g.manual_position, g.id",
            )?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let id = row.get(0)?;
                if row.get::<_, bool>(1)? {
                    slots.push(ordered.len());
                    visible.push(id);
                }
                ordered.push(id);
            }
        }
        if !movement.apply(&mut visible) {
            return Err("The dragged game or its destination is no longer in the library".into());
        }
        // Grouping or removing a game can temporarily hide its card. Keep its
        // slot so separating the discs or adding the game back restores it.
        for (slot, id) in slots.into_iter().zip(visible) {
            ordered[slot] = id;
        }
        tx.execute(
            "UPDATE games SET manual_position = r.key FROM json_each(?1) r
             WHERE games.id = r.value AND games.manual_position IS NOT r.key",
            [serde_json::to_string(&ordered)?],
        )?;
        tx.commit()?;
        Ok(ordered)
    }
}

#[cfg(test)]
mod tests {
    use super::{GameMove, Placement};

    #[test]
    fn drops_on_either_half_of_the_target_work_in_both_directions() {
        for (source, target, placement, expected) in [
            (1, 3, Placement::Before, [2, 1, 3]),
            (1, 3, Placement::After, [2, 3, 1]),
            (3, 1, Placement::Before, [3, 1, 2]),
            (3, 1, Placement::After, [1, 3, 2]),
            (2, 1, Placement::After, [1, 2, 3]),
            (1, 2, Placement::Before, [1, 2, 3]),
        ] {
            let mut ids = [1, 2, 3];
            assert!(
                GameMove {
                    source,
                    target,
                    placement,
                    initial_order: vec![]
                }
                .apply(&mut ids)
            );
            assert_eq!(ids, expected);
        }
    }
}
