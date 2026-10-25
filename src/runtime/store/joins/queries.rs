//! Indexed join lifecycle and membership queries.
pub(super) const SNAPSHOT: &str =
    "SELECT id, requester, request_id, created_at, deadline, state FROM joins WHERE id=?1";
pub(super) const MEMBERS: &str = "SELECT position, message_id, target, outcome, output, \
    output_final, task_completed, delivered FROM join_members WHERE join_id=?1 ORDER BY \
    position";
pub(super) const OPEN: &str = "SELECT id FROM joins WHERE requester=?1 AND state IN \
    ('collecting','deadline_passed') ORDER BY id";
/// A join still collecting before its deadline, or a join result still waiting
/// to be delivered. After a deadline the requester may decide to stop waiting.
pub(super) const BLOCKS_COMPLETION: &str = "SELECT EXISTS(SELECT 1 FROM joins WHERE \
    requester=?1 AND state='collecting') OR EXISTS(SELECT 1 FROM messages WHERE recipient=?1 \
    AND kind='join_result' AND state='accepted')";
pub(super) const NEXT_DEADLINE: &str = concat!(
    "SELECT min(deadline) FROM joins WHERE state='collecting' AND deadline IS NOT NULL",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id = joins.requester AND \
            task_outcome='open' AND expired=0), 0)"
    ),
    ""
);
pub(super) const REQUEST_USED: &str =
    "SELECT EXISTS(SELECT 1 FROM requests WHERE sender=?1 AND id=?2)";
pub(super) const CREATE: &str = "INSERT INTO \
    joins(id,requester,request_id,created_at,deadline,state) \
    VALUES(?1,?2,?3,?4,?5,'collecting')";
pub(super) const ADD_MEMBER: &str = "\nINSERT INTO \
    join_members(join_id,requester,position,message_id,target,outcome)\nSELECT \
    ?1,?2,?3,id,recipient,CASE WHEN state IN \
    ('completed','failed','interrupted','undeliverable','uncertain') THEN \
    state ELSE 'pending' END\nFROM messages WHERE id=?4 AND join_id IS NULL \
    AND mode='next-turn' AND redirect=0 AND recipient != ?2";
pub(super) const ATTACH: &str = "UPDATE messages SET join_id=?1 WHERE id=?2";
pub(super) const TRACK: &str = "INSERT INTO call_results(message_id,accepted_at) VALUES(?1,?2) \
    ON CONFLICT(message_id) DO NOTHING";
pub(super) const DIRTY: &str = concat!(
    "SELECT id FROM joins WHERE dirty=1",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id = joins.requester AND \
            task_outcome='open' AND expired=0), 0)"
    ),
    " ORDER BY id LIMIT 128"
);
pub(super) const DUE: &str = concat!(
    "SELECT id FROM joins WHERE state='collecting' AND deadline IS NOT NULL AND deadline<=?1",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id = joins.requester AND \
            task_outcome='open' AND expired=0), 0)"
    ),
    " ORDER BY deadline,id LIMIT 128"
);
pub(super) const CLEAR_DIRTY: &str = "UPDATE joins SET dirty=0 WHERE id=?1";
pub(super) const STATE: &str = "UPDATE joins SET state=?1 WHERE id=?2";
pub(super) const RESULT_KIND: &str = "UPDATE messages SET kind='join_result' WHERE id=?1";
pub(super) const DELIVERED: &str = "UPDATE join_members SET delivered=1 WHERE join_id=?1 AND \
    outcome != 'pending' AND delivered=0";

pub(super) const SUMMARY: &str = r"
SELECT j.requester, j.request_id, j.state, count(m.message_id),
       count(CASE WHEN m.outcome = 'pending' THEN 1 END)
FROM joins j LEFT JOIN join_members m ON m.join_id = j.id
WHERE j.id = ?1 GROUP BY j.id";

pub(super) const OUTPUT_OWNER: &str = r"
SELECT j.id, j.requester, m.output
FROM joins j JOIN join_members m ON m.join_id = j.id
WHERE m.message_id = ?1 AND m.outcome = 'pending'
  AND j.state IN ('collecting', 'deadline_passed')";
pub(super) const COMPLETION_OWNER: &str = r"
SELECT j.id, j.requester, m.output, m.task_completed
FROM joins j JOIN join_members m ON m.join_id = j.id
WHERE m.message_id = ?1 AND m.target = ?2 AND m.outcome = 'completed'
  AND j.state IN ('collecting', 'deadline_passed')";
pub(super) const SET_OUTPUT: &str =
    "UPDATE join_members SET output=?1, output_final=?2 WHERE message_id=?3";
pub(super) const SET_COMPLETION: &str =
    "UPDATE join_members SET task_completed=?1 WHERE message_id=?2";
pub(super) const RETIRE_OUTPUT: &str =
    "DELETE FROM artifacts WHERE id=?1 AND join_id=?2 AND coalesce(agent, 'human')=?3";
