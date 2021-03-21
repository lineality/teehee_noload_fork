use super::byte_rope::{Rope, RopeDelta};
use xi_rope::delta::DeltaElement;
use xi_rope::multiset::Subset;

struct Action {
    delta: RopeDelta,
}

impl Action {
    fn from_delta(delta: RopeDelta) -> Action {
        Action { delta }
    }
    fn invert(&self, base_rope: &Rope) -> Action {
        let (inserts, deletions) = self.delta.clone().factor();
        let ins_subset = inserts.inserted_subset();
        let deleted_now = base_rope.without_subset(deletions.complement());

        let deletions_from_base = deletions.transform_expand(&ins_subset);
        let deletions_from_inverted = ins_subset;
        Action {
            delta: RopeDelta::synthesize(
                &deleted_now.into_node(),
                &deletions_from_base,
                &deletions_from_inverted,
            ),
        }
    }
    fn subsets_for_chain(self, next: RopeDelta) -> (Subset, Subset, Subset) {
        let (ins1, del1) = self.delta.factor();
        let (ins2, del2) = next.factor();

        let inserts_in_mid_union = ins1.inserted_subset();
        let deletes_from_mid_union = del1.transform_expand(&ins1.inserted_subset());

        let union_ins_delta = ins2.transform_expand(&deletes_from_mid_union, true);
        let new_inserts = union_ins_delta.inserted_subset();
        let new_deletes = {
            let pre_expand = del2.transform_expand(&deletes_from_mid_union);
            if new_inserts.is_empty() {
                pre_expand
            } else {
                pre_expand.transform_expand(&new_inserts)
            }
        };
        let rebased_deletes_from_union = deletes_from_mid_union.transform_expand(&new_inserts);
        let deletes_from_union = rebased_deletes_from_union.union(&new_deletes);

        let rebased_inserts_in_union = inserts_in_mid_union.transform_expand(&new_inserts);
        let inserts_in_union = rebased_inserts_in_union.union(&new_inserts);

        let inserts_in_mid_text = inserts_in_mid_union.transform_shrink(&deletes_from_mid_union);
        let prefinal_insertion = ins2.inserted_subset();
        let deletes_from_prefinal = del2.transform_expand(&prefinal_insertion);
        let inserts_in_prefinal = inserts_in_mid_text.transform_union(&prefinal_insertion);

        // (inserts, deletes, inserts_in_prefinal)
        (inserts_in_union, deletes_from_union, inserts_in_prefinal)

    }

    fn chain(self, after_self: &Rope, next: RopeDelta) -> Action {
        let after_next = after_self.apply_delta(&next.clone().factor().0); // don't do prefinal deletions
        let (inserted, deleted, inserts_in_prefinal) = self.subsets_for_chain(next);

        let tombstones = dbg!(after_next.without_subset(inserts_in_prefinal.complement()));
        dbg!(&inserted);
        dbg!(&deleted);

        Action {
            delta: RopeDelta::synthesize(
                &tombstones.into_node(),
                &inserted,
                &deleted,
            ),
        }
    }
}

struct History {
    current_incomplete: Option<Action>,

    undo: Vec<Action>,
    redo: Vec<Action>,
}

impl History {
    fn commit(&mut self) {}
}

#[cfg(test)]
mod test {
    use super::*;
    use xi_rope::delta::{Delta, DeltaElement};
    use xi_rope::DeltaBuilder;

    #[test]
    fn test_delete() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.delete(0..1);
        let deletion = delta_builder.build();
        let inversion = Action {
            delta: deletion.clone(),
        }
        .invert(&base_rope);

        let erased_rope = base_rope.apply_delta(&deletion);
        assert_eq!(&erased_rope.slice_to_cow(..), &vec![1, 2, 3]);
        let unerased_rope = erased_rope.apply_delta(&inversion.delta);
        assert_eq!(&unerased_rope.slice_to_cow(..), &vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_middle_delete() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.delete(1..3);
        let deletion = delta_builder.build();
        let inversion = Action {
            delta: deletion.clone(),
        }
        .invert(&base_rope);

        let erased_rope = base_rope.apply_delta(&deletion);
        assert_eq!(&erased_rope.slice_to_cow(..), &vec![0, 3]);
        let unerased_rope = erased_rope.apply_delta(&inversion.delta);
        assert_eq!(&unerased_rope.slice_to_cow(..), &vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_insert() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.replace(1..1, Into::<Rope>::into(vec![5]).into_node());
        let insertion = delta_builder.build();
        let inversion = Action {
            delta: insertion.clone(),
        }
        .invert(&base_rope);

        let inserted_rope = base_rope.apply_delta(&insertion);
        assert_eq!(&inserted_rope.slice_to_cow(..), &vec![0, 5, 1, 2, 3]);
        let uninserted_rope = inserted_rope.apply_delta(&inversion.delta);
        assert_eq!(&uninserted_rope.slice_to_cow(..), &vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_replace() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.replace(1..2, Into::<Rope>::into(vec![5, 6]).into_node());
        let sub = delta_builder.build();
        let inversion = Action { delta: sub.clone() }.invert(&base_rope);

        let replaced_rope = base_rope.apply_delta(&sub);
        assert_eq!(&replaced_rope.slice_to_cow(..), &vec![0, 5, 6, 2, 3]);
        let unreplaced_rope = replaced_rope.apply_delta(&inversion.delta);
        assert_eq!(&unreplaced_rope.slice_to_cow(..), &vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_chain_delete() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.delete(0..1);
        let deletion1 = delta_builder.build();
        let mid_rope = base_rope.apply_delta(&deletion1);

        let mut delta_builder2 = DeltaBuilder::new(mid_rope.len());
        delta_builder2.delete(0..1);
        let deletion2 = delta_builder2.build();
        let final_rope = mid_rope.apply_delta(&deletion2);

        assert_eq!(&final_rope.slice_to_cow(..), &vec![2, 3]);
        let chained_subsets = Action::from_delta(deletion1.clone()).subsets_for_chain(deletion2.clone());
        assert_eq!(&chained_subsets.0.delete_from_string("0123"), "0123");
        assert_eq!(&chained_subsets.1.delete_from_string("0123"), "23");
        assert_eq!(&chained_subsets.2.delete_from_string("123"), "123");

        let chained_delta = Action::from_delta(deletion1).chain(&mid_rope, deletion2);
        let chain_final_rope = base_rope.apply_delta(&chained_delta.delta);
        assert_eq!(&chain_final_rope.slice_to_cow(..), &vec![2, 3]);
    }

    #[test]
    fn test_chain_delete_insert() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.delete(0..1);
        let deletion1 = delta_builder.build();
        let mid_rope = base_rope.apply_delta(&deletion1);

        let mut delta_builder2 = DeltaBuilder::new(mid_rope.len());
        delta_builder2.replace(1..1, Into::<Rope>::into(vec![5, 6]).into_node());
        let insertion = delta_builder2.build();
        let final_rope = mid_rope.apply_delta(&insertion);

        assert_eq!(&final_rope.slice_to_cow(..), &vec![1, 5, 6, 2, 3]);
        let chained_subsets = Action::from_delta(deletion1.clone()).subsets_for_chain(insertion.clone());
        assert_eq!(&chained_subsets.0.delete_from_string("015623"), "0123");
        assert_eq!(&chained_subsets.1.delete_from_string("015623"), "15623");
        assert_eq!(&chained_subsets.2.delete_from_string("15623"), "123");

        let chained_delta = Action::from_delta(deletion1).chain(&mid_rope, insertion);
        let chain_final_rope = base_rope.apply_delta(&chained_delta.delta);
        assert_eq!(&chain_final_rope.slice_to_cow(..), &vec![1, 5, 6, 2, 3]);
    }

    #[test]
    fn test_chain_insert_delete() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.replace(0..0, Into::<Rope>::into(vec![5, 6, 7]).into_node());
        let insertion = delta_builder.build();
        let mid_rope = base_rope.apply_delta(&insertion);

        let mut delta_builder2 = DeltaBuilder::new(mid_rope.len());
        delta_builder2.delete(0..1);
        let deletion = delta_builder2.build();
        let final_rope = mid_rope.apply_delta(&deletion);

        assert_eq!(&final_rope.slice_to_cow(..), &vec![6, 7, 0, 1, 2, 3]);
        let chained_subsets = Action::from_delta(insertion.clone()).subsets_for_chain(deletion.clone());
        assert_eq!(&chained_subsets.0.delete_from_string("5670123"), "0123");
        assert_eq!(&chained_subsets.1.delete_from_string("5670123"), "670123");
        assert_eq!(&chained_subsets.2.delete_from_string("5670123"), "0123");

        let chained_delta = Action::from_delta(insertion).chain(&mid_rope, deletion);
        let chain_final_rope = base_rope.apply_delta(&chained_delta.delta);
        assert_eq!(&chain_final_rope.slice_to_cow(..), &vec![6, 7, 0, 1, 2, 3]);
    }

    #[test]
    fn test_chain_insert() {
        let base_rope: Rope = vec![0, 1, 2, 3].into();
        let mut delta_builder = DeltaBuilder::new(base_rope.len());
        delta_builder.replace(1..1, Into::<Rope>::into(vec![5]).into_node());
        let insertion1 = delta_builder.build();
        let mid_rope = base_rope.apply_delta(&insertion1);

        let mut delta_builder2 = DeltaBuilder::new(mid_rope.len());
        delta_builder2.replace(2..2, Into::<Rope>::into(vec![6]).into_node());
        let insertion2 = delta_builder2.build();
        let final_rope = mid_rope.apply_delta(&insertion2);

        assert_eq!(&final_rope.slice_to_cow(..), &vec![0, 5, 6, 1, 2, 3]);
        let chained_subsets = Action::from_delta(insertion1.clone()).subsets_for_chain(insertion2.clone());
        assert_eq!(&chained_subsets.0.delete_from_string("056123"), "0123");
        assert_eq!(&chained_subsets.1.delete_from_string("056123"), "056123");
        assert_eq!(&chained_subsets.2.delete_from_string("056123"), "0123");

        let chained_delta = Action::from_delta(insertion1).chain(&mid_rope, insertion2);
        let chain_final_rope = base_rope.apply_delta(&chained_delta.delta);
        assert_eq!(&chain_final_rope.slice_to_cow(..), &vec![0, 5, 6, 1, 2, 3]);
    }
}