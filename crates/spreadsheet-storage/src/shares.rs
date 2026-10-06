use crate::{CryptoError, PgDatabase};
use serde::Serialize;
use sha2::{Digest, Sha256};
use spreadsheet_application::journal::{AuthenticatedActor, DocumentAccess, JournalError};
use spreadsheet_domain::workbook::{ActorId, DocumentId, DocumentVersion, WorkspaceId};
use sqlx::{AssertSqlSafe, FromRow, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_PHC_BYTES: usize = 256;
const MAX_LIST: u32 = 100;
const ATTEMPT_NAMESPACE: i32 = 1_347_962_189;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareRole {
    View,
    Comment,
    Edit,
}
impl ShareRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Comment => "comment",
            Self::Edit => "edit",
        }
    }
    fn parse(value: &str) -> Result<Self, ShareError> {
        match value {
            "view" => Ok(Self::View),
            "comment" => Ok(Self::Comment),
            "edit" => Ok(Self::Edit),
            _ => Err(ShareError::Corrupt),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareSecretKind {
    Pin,
    Password,
}
impl ShareSecretKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pin => "pin",
            Self::Password => "password",
        }
    }
    fn parse(value: &str) -> Result<Self, ShareError> {
        match value {
            "pin" => Ok(Self::Pin),
            "password" => Ok(Self::Password),
            _ => Err(ShareError::Corrupt),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShareLifetime {
    Hour,
    Day,
    Week,
    Indefinite { confirmed: bool },
}
impl ShareLifetime {
    fn seconds(self) -> Result<Option<i64>, ShareError> {
        match self {
            Self::Hour => Ok(Some(3600)),
            Self::Day => Ok(Some(86400)),
            Self::Week => Ok(Some(604800)),
            Self::Indefinite { confirmed: true } => Ok(None),
            Self::Indefinite { confirmed: false } => Err(ShareError::InvalidInput),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareStatus {
    Active,
    Revoked,
    Expired,
    Disabled,
}
impl ShareStatus {
    fn parse(value: &str) -> Result<Self, ShareError> {
        match value {
            "active" => Ok(Self::Active),
            "revoked" => Ok(Self::Revoked),
            "expired" => Ok(Self::Expired),
            "disabled" => Ok(Self::Disabled),
            _ => Err(ShareError::Corrupt),
        }
    }
}

/// PHC text is already hashed by the bounded server Argon worker.
/// The wrapper has no Debug implementation because its text is a credential.
pub struct ShareSecretPhc {
    kind: ShareSecretKind,
    text: Zeroizing<String>,
}
impl ShareSecretPhc {
    pub fn new(kind: ShareSecretKind, text: Zeroizing<String>) -> Result<Self, ShareError> {
        validate_phc(&text)?;
        Ok(Self { kind, text })
    }
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
fn base64_length(value: &str) -> Option<usize> {
    if value.is_empty() || value.len() % 4 == 1 {
        return None;
    }
    if !value.bytes().all(|byte| base64_value(byte).is_some()) {
        return None;
    }
    let last = base64_value(*value.as_bytes().last()?)?;
    if (value.len() % 4 == 2 && last & 15 != 0) || (value.len() % 4 == 3 && last & 3 != 0) {
        return None;
    }
    Some(value.len() * 6 / 8)
}
fn validate_phc(value: &str) -> Result<(), ShareError> {
    if value.len() > MAX_PHC_BYTES {
        return Err(ShareError::InvalidInput);
    }
    let mut fields = value.split('$');
    if fields.next() != Some("")
        || fields.next() != Some("argon2id")
        || fields.next() != Some("v=19")
        || fields.next() != Some("m=65536,t=3,p=1")
    {
        return Err(ShareError::InvalidInput);
    }
    let salt = fields.next().ok_or(ShareError::InvalidInput)?;
    let hash = fields.next().ok_or(ShareError::InvalidInput)?;
    if !matches!(base64_length(salt), Some(16..=64))
        || base64_length(hash) != Some(32)
        || fields.next().is_some()
    {
        return Err(ShareError::InvalidInput);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ShareError {
    #[error("share input is invalid")]
    InvalidInput,
    #[error("share access denied")]
    Unauthorized,
    #[error("share document or link not found")]
    NotFound,
    #[error("share request identifier reused with different intent")]
    IdempotencyConflict,
    #[error("share operation conflicted; retry")]
    Conflict,
    #[error("share storage unavailable")]
    Unavailable,
    #[error("share storage contained invalid data")]
    Corrupt,
}
impl From<sqlx::Error> for ShareError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::Database(error)
                if matches!(
                    error.code().as_deref(),
                    Some("40001" | "40P01" | "55P03" | "23505")
                ) =>
            {
                Self::Conflict
            }
            _ => Self::Unavailable,
        }
    }
}
impl From<CryptoError> for ShareError {
    fn from(error: CryptoError) -> Self {
        match error {
            CryptoError::Database(_) | CryptoError::RestorePending => Self::Unavailable,
            _ => Self::Corrupt,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShareLinkSummary {
    pub anonymized_at_creation: bool,
    pub link_id: Uuid,
    pub document_id: DocumentId,
    pub role: ShareRole,
    pub secret_kind: ShareSecretKind,
    pub status: ShareStatus,
    pub expires_at_unix: Option<i64>,
    pub created_at_unix: i64,
    pub failed_attempts: u32,
    pub lockouts: u32,
    pub blocked_remaining_seconds: u64,
}
pub struct ShareCreateReceipt {
    pub link: ShareLinkSummary,
    pub newly_committed: bool,
    pub token: Option<Zeroizing<[u8; 32]>>,
}
#[derive(Clone, Copy, Debug)]
pub struct ShareMutationReceipt {
    pub document_id: DocumentId,
    pub access_epoch: u64,
    pub newly_committed: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct GuestSession {
    pub session_id: Uuid,
    pub link_id: Uuid,
    pub document_id: DocumentId,
    pub identity: AuthenticatedActor,
    pub role: ShareRole,
    pub remaining_seconds: u64,
    pub remaining_millis: u64,
    pub admitted_head_version: DocumentVersion,
}
pub struct GuestSessionSeed {
    pub session_id: Uuid,
    pub actor_id: ActorId,
    pub token_hash: [u8; 32],
}
#[derive(Debug)]
pub enum ShareAttemptOutcome {
    Accepted(GuestSession),
    Invalid,
    Invalidated(ShareMutationReceipt),
    Blocked { remaining_seconds: u64 },
}
#[derive(Clone, Debug)]
pub struct ShareOwnerNotification {
    pub notification_id: Uuid,
    pub document_id: DocumentId,
    pub link_id: Uuid,
    pub reason: String,
    pub created_at_unix: i64,
}

#[derive(FromRow)]
struct LinkRow {
    anonymized_at_creation: bool,
    link_id: Uuid,
    workspace_id: Uuid,
    document_id: Uuid,
    owner_id: Uuid,
    token_hash: Vec<u8>,
    role: String,
    secret_kind: String,
    secret_phc: Vec<u8>,
    secret_intent: Vec<u8>,
    status: String,
    failed_attempts: i32,
    lockouts: i32,
    expires_at_unix: Option<i64>,
    created_at_unix: i64,
    blocked_remaining_seconds: i64,
    expired: bool,
}
const LINK_SELECT: &str = "SELECT anonymized_at_creation,link_id,workspace_id,document_id,owner_id,token_hash,role,secret_kind,secret_phc,secret_intent,status,failed_attempts,lockouts,floor(extract(epoch FROM expires_at))::bigint AS expires_at_unix,floor(extract(epoch FROM created_at))::bigint AS created_at_unix,GREATEST(0,ceil(extract(epoch FROM blocked_until-clock_timestamp())))::bigint AS blocked_remaining_seconds,(expires_at IS NOT NULL AND expires_at<=clock_timestamp()) AS expired FROM share_links";
impl LinkRow {
    fn summary(&self) -> Result<ShareLinkSummary, ShareError> {
        Ok(ShareLinkSummary {
            anonymized_at_creation: self.anonymized_at_creation,
            link_id: self.link_id,
            document_id: document_id(self.document_id)?,
            role: ShareRole::parse(&self.role)?,
            secret_kind: ShareSecretKind::parse(&self.secret_kind)?,
            status: if self.status == "active" && self.expired {
                ShareStatus::Expired
            } else {
                ShareStatus::parse(&self.status)?
            },
            expires_at_unix: self.expires_at_unix,
            created_at_unix: self.created_at_unix,
            failed_attempts: u32::try_from(self.failed_attempts)
                .map_err(|_| ShareError::Corrupt)?,
            lockouts: u32::try_from(self.lockouts).map_err(|_| ShareError::Corrupt)?,
            blocked_remaining_seconds: u64::try_from(self.blocked_remaining_seconds)
                .map_err(|_| ShareError::Corrupt)?,
        })
    }
}
#[derive(FromRow)]
struct DocumentRow {
    document_id: Uuid,
    workspace_id: Uuid,
    head_version: i64,
    access_epoch: i64,
    state: String,
}
#[derive(FromRow)]
struct SessionRow {
    session_id: Uuid,
    link_id: Uuid,
    document_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    role: String,
    remaining_millis: i64,
    admitted_head_version: i64,
}
impl SessionRow {
    fn session(self) -> Result<GuestSession, ShareError> {
        let remaining_millis =
            u64::try_from(self.remaining_millis).map_err(|_| ShareError::Corrupt)?;
        Ok(GuestSession {
            session_id: self.session_id,
            link_id: self.link_id,
            document_id: document_id(self.document_id)?,
            identity: AuthenticatedActor {
                actor_id: ActorId::from_bytes(*self.user_id.as_bytes())
                    .map_err(|_| ShareError::Corrupt)?,
                workspace_id: WorkspaceId::from_bytes(*self.workspace_id.as_bytes())
                    .map_err(|_| ShareError::Corrupt)?,
            },
            role: ShareRole::parse(&self.role)?,
            remaining_seconds: remaining_millis / 1000,
            remaining_millis,
            admitted_head_version: document_version(self.admitted_head_version)?,
        })
    }
}
const SESSION_SELECT: &str = "SELECT s.session_id,s.link_id,s.document_id,l.workspace_id,s.user_id,l.role,GREATEST(0,floor(extract(epoch FROM LEAST(s.expires_at,l.expires_at)-clock_timestamp())*1000))::bigint AS remaining_millis,s.admitted_head_version FROM share_guest_sessions s JOIN share_links l ON l.link_id=s.link_id AND l.document_id=s.document_id JOIN users u ON u.user_id=s.user_id AND u.identity_kind='share_guest' WHERE s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND l.status='active' AND (l.expires_at IS NULL OR l.expires_at>clock_timestamp())";
fn id(bytes: &[u8; 16]) -> Uuid {
    Uuid::from_bytes(*bytes)
}
fn document_id(value: Uuid) -> Result<DocumentId, ShareError> {
    DocumentId::from_bytes(*value.as_bytes()).map_err(|_| ShareError::Corrupt)
}
fn document_version(value: i64) -> Result<DocumentVersion, ShareError> {
    let value = u64::try_from(value).map_err(|_| ShareError::Corrupt)?;
    if value == 0 {
        Ok(DocumentVersion::ZERO)
    } else {
        DocumentVersion::new(value).map_err(|_| ShareError::Corrupt)
    }
}
fn equal_bytes(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (*a ^ *b))
        == 0
}
async fn lock_document(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
) -> Result<DocumentRow, ShareError> {
    sqlx::query_as("SELECT document_id,workspace_id,head_version,access_epoch,state FROM documents WHERE document_id=$1 FOR UPDATE")
        .bind(id(document.as_bytes())).fetch_optional(&mut **tx).await?.ok_or(ShareError::NotFound)
}
async fn require_owner(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    doc: &DocumentRow,
) -> Result<(), ShareError> {
    let allowed=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM users u JOIN memberships m ON m.user_id=u.user_id JOIN document_permissions p ON p.user_id=u.user_id AND p.workspace_id=m.workspace_id WHERE u.user_id=$1 AND u.identity_kind='member' AND m.workspace_id=$2 AND p.document_id=$3 AND p.role='owner')")
        .bind(id(identity.actor_id.as_bytes())).bind(id(identity.workspace_id.as_bytes())).bind(doc.document_id).fetch_one(&mut **tx).await?;
    if !allowed || doc.workspace_id != id(identity.workspace_id.as_bytes()) {
        Err(ShareError::Unauthorized)
    } else if doc.state != "active" {
        Err(ShareError::Unavailable)
    } else {
        Ok(())
    }
}
async fn fetch_link(
    tx: &mut Transaction<'_, Postgres>,
    link: Uuid,
    locked: bool,
) -> Result<Option<LinkRow>, ShareError> {
    let query = format!(
        "{LINK_SELECT} WHERE link_id=$1{}",
        if locked { " FOR UPDATE" } else { "" }
    );
    Ok(sqlx::query_as(AssertSqlSafe(query))
        .bind(link)
        .fetch_optional(&mut **tx)
        .await?)
}
async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    link: &LinkRow,
    actor: Option<Uuid>,
    request: Option<Uuid>,
    event_type: &str,
    epoch: i64,
) -> Result<Uuid, ShareError> {
    let event = Uuid::now_v7();
    sqlx::query("INSERT INTO share_audit_events(event_id,link_id,document_id,actor_id,request_id,event_type,access_epoch) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(event).bind(link.link_id).bind(link.document_id).bind(actor).bind(request).bind(event_type).bind(epoch).execute(&mut **tx).await?;
    Ok(event)
}
async fn terminal_link(
    tx: &mut Transaction<'_, Postgres>,
    link: &LinkRow,
    status: &str,
    event_type: &str,
    actor: Option<Uuid>,
    request: Option<Uuid>,
) -> Result<i64, ShareError> {
    let changed = sqlx::query(
        "UPDATE share_links SET status=$2,blocked_until=NULL WHERE link_id=$1 AND status='active'",
    )
    .bind(link.link_id)
    .bind(status)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(ShareError::Conflict);
    }
    sqlx::query("UPDATE share_guest_sessions SET revoked_at=clock_timestamp() WHERE link_id=$1 AND revoked_at IS NULL")
        .bind(link.link_id).execute(&mut **tx).await?;
    let epoch=sqlx::query_scalar::<_,i64>("UPDATE documents SET access_epoch=access_epoch+1 WHERE document_id=$1 AND access_epoch<9223372036854775807 RETURNING access_epoch")
        .bind(link.document_id).fetch_optional(&mut **tx).await?.ok_or(ShareError::Corrupt)?;
    let event = audit(tx, link, actor, request, event_type, epoch).await?;
    if status == "disabled" {
        sqlx::query("INSERT INTO share_owner_notifications(notification_id,event_id,user_id,document_id,link_id,reason) SELECT gen_random_uuid(),$1,p.user_id,$2,$3,'disabled' FROM document_permissions p JOIN users u ON u.user_id=p.user_id AND u.identity_kind='member' JOIN memberships m ON m.user_id=p.user_id AND m.workspace_id=p.workspace_id WHERE p.document_id=$2 AND p.role='owner'")
            .bind(event).bind(link.document_id).bind(link.link_id).execute(&mut **tx).await?;
    }
    Ok(epoch)
}

impl PgDatabase {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_share(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        request_id: Uuid,
        role: ShareRole,
        lifetime: ShareLifetime,
        secret: ShareSecretPhc,
        secret_intent_hash: [u8; 32],
    ) -> Result<ShareCreateReceipt, ShareError> {
        self.create_share_presented(
            identity,
            document,
            request_id,
            role,
            lifetime,
            secret,
            secret_intent_hash,
            false,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn create_share_presented(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        request_id: Uuid,
        role: ShareRole,
        lifetime: ShareLifetime,
        secret: ShareSecretPhc,
        secret_intent_hash: [u8; 32],
        anonymized: bool,
    ) -> Result<ShareCreateReceipt, ShareError> {
        if anonymized && role == ShareRole::Edit {
            return Err(ShareError::InvalidInput);
        }
        let secret_intent_hash = Zeroizing::new(secret_intent_hash);
        if request_id.is_nil() {
            return Err(ShareError::InvalidInput);
        }
        let lifetime = lifetime.seconds()?;
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let doc = lock_document(&mut tx, document).await?;
        require_owner(&mut tx, identity, &doc).await?;
        let prior_query = format!("{LINK_SELECT} WHERE create_request_id=$1");
        if let Some(prior) = sqlx::query_as::<_, LinkRow>(AssertSqlSafe(prior_query))
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await?
        {
            if prior.document_id != doc.document_id
                || prior.owner_id != id(identity.actor_id.as_bytes())
                || prior.role != role.as_str()
                || prior.anonymized_at_creation != anonymized
                || prior.secret_kind != secret.kind.as_str()
                || prior
                    .expires_at_unix
                    .map(|expires| expires - prior.created_at_unix)
                    != lifetime
            {
                return Err(ShareError::IdempotencyConflict);
            }
            let intent = self
                .open_record(
                    document,
                    1,
                    "share-intent",
                    prior.link_id,
                    1,
                    &prior.secret_intent,
                    32,
                )
                .await?;
            if !equal_bytes(&intent, secret_intent_hash.as_slice()) {
                return Err(ShareError::IdempotencyConflict);
            }
            let link = prior.summary()?;
            tx.commit().await?;
            return Ok(ShareCreateReceipt {
                link,
                newly_committed: false,
                token: None,
            });
        }
        let reused = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM share_audit_events WHERE actor_id=$1 AND request_id=$2)",
        )
        .bind(id(identity.actor_id.as_bytes()))
        .bind(request_id)
        .fetch_one(&mut *tx)
        .await?;
        if reused {
            return Err(ShareError::IdempotencyConflict);
        }
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM share_links WHERE document_id=$1 AND status='active'",
        )
        .bind(doc.document_id)
        .fetch_one(&mut *tx)
        .await?;
        if count >= i64::from(MAX_LIST) {
            return Err(ShareError::InvalidInput);
        }
        let link_id = Uuid::now_v7();
        if anonymized {
            let privacy = self
                .read_privacy_policy(&mut tx, document, 0)
                .await
                .map_err(|_| ShareError::Unavailable)?;
            if !privacy.rules.enabled
                || privacy
                    .rules
                    .kinds()
                    .map_err(|_| ShareError::InvalidInput)?
                    .is_empty()
            {
                return Err(ShareError::InvalidInput);
            }
        }
        let mut token = Zeroizing::new([0_u8; 32]);
        getrandom::fill(token.as_mut()).map_err(|_| ShareError::Unavailable)?;
        let token_hash = Sha256::digest(token.as_slice());
        let phc = self
            .seal_record_guarded(
                document,
                1,
                "share-secret",
                link_id,
                1,
                secret.text.as_bytes(),
                MAX_PHC_BYTES,
            )
            .await?;
        let intent = self
            .seal_record_guarded(
                document,
                1,
                "share-intent",
                link_id,
                1,
                secret_intent_hash.as_slice(),
                32,
            )
            .await?;
        sqlx::query("WITH t AS (SELECT clock_timestamp() AS now) INSERT INTO share_links(link_id,workspace_id,document_id,owner_id,create_request_id,token_hash,role,secret_kind,secret_phc,secret_intent,created_at,expires_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,t.now,CASE WHEN $11::bigint IS NULL THEN NULL ELSE t.now+($11::bigint*interval '1 second') END FROM t")
            .bind(link_id).bind(doc.workspace_id).bind(doc.document_id).bind(id(identity.actor_id.as_bytes())).bind(request_id).bind(token_hash.as_slice()).bind(role.as_str()).bind(secret.kind.as_str()).bind(phc).bind(intent).bind(lifetime).execute(&mut *tx).await?;
        if anonymized {
            sqlx::query("UPDATE share_links SET anonymized_at_creation=true WHERE link_id=$1")
                .bind(link_id)
                .execute(&mut *tx)
                .await?;
            let mut rules = crate::DisclosureRules::defaults(true);
            rules.require_anonymization = true;
            let bytes =
                Zeroizing::new(serde_json::to_vec(&rules).map_err(|_| ShareError::Corrupt)?);
            let payload = self
                .seal_record_guarded(document, 1, "disclosure-policy", link_id, 1, &bytes, 4096)
                .await?;
            sqlx::query("INSERT INTO disclosure_policies(scope_id,document_id,link_id,revision,payload) VALUES($1,$2,$1,1,$3)").bind(link_id).bind(doc.document_id).bind(payload).execute(&mut *tx).await?;
        }
        let row = fetch_link(&mut tx, link_id, false)
            .await?
            .ok_or(ShareError::Corrupt)?;
        audit(
            &mut tx,
            &row,
            Some(id(identity.actor_id.as_bytes())),
            Some(request_id),
            "created",
            doc.access_epoch,
        )
        .await?;
        let link = row.summary()?;
        tx.commit().await?;
        Ok(ShareCreateReceipt {
            link,
            newly_committed: true,
            token: Some(token),
        })
    }

    pub async fn list_shares(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        limit: u32,
    ) -> Result<Vec<ShareLinkSummary>, ShareError> {
        if !(1..=MAX_LIST).contains(&limit) {
            return Err(ShareError::InvalidInput);
        }
        let mut tx = self.begin_guarded().await?;
        let doc = lock_document(&mut tx, document).await?;
        require_owner(&mut tx, identity, &doc).await?;
        let query = format!(
            "{LINK_SELECT} WHERE document_id=$1 ORDER BY (status='active') DESC,created_at DESC,link_id DESC LIMIT $2"
        );
        let rows = sqlx::query_as::<_, LinkRow>(AssertSqlSafe(query))
            .bind(doc.document_id)
            .bind(i64::from(limit))
            .fetch_all(&mut *tx)
            .await?;
        let result = rows
            .iter()
            .map(LinkRow::summary)
            .collect::<Result<Vec<_>, _>>()?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn revoke_share(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        link_id: Uuid,
        request_id: Uuid,
    ) -> Result<ShareMutationReceipt, ShareError> {
        if link_id.is_nil() || request_id.is_nil() {
            return Err(ShareError::InvalidInput);
        }
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let doc = lock_document(&mut tx, document).await?;
        require_owner(&mut tx, identity, &doc).await?;
        let link = fetch_link(&mut tx, link_id, true)
            .await?
            .filter(|row| row.document_id == doc.document_id)
            .ok_or(ShareError::NotFound)?;
        if let Some((prior_link,prior_doc,prior_type,epoch))=sqlx::query_as::<_,(Uuid,Uuid,String,i64)>("SELECT link_id,document_id,event_type,access_epoch FROM share_audit_events WHERE actor_id=$1 AND request_id=$2")
            .bind(id(identity.actor_id.as_bytes())).bind(request_id).fetch_optional(&mut *tx).await? {
            if prior_link!=link_id||prior_doc!=doc.document_id||prior_type!="revoked"{return Err(ShareError::IdempotencyConflict);}
            tx.commit().await?;
            return Ok(ShareMutationReceipt{document_id:document,access_epoch:u64::try_from(epoch).map_err(|_|ShareError::Corrupt)?,newly_committed:false});
        }
        let changed = link.status == "active";
        let epoch = if changed {
            terminal_link(
                &mut tx,
                &link,
                "revoked",
                "revoked",
                Some(id(identity.actor_id.as_bytes())),
                Some(request_id),
            )
            .await?
        } else {
            audit(
                &mut tx,
                &link,
                Some(id(identity.actor_id.as_bytes())),
                Some(request_id),
                "revoked",
                doc.access_epoch,
            )
            .await?;
            doc.access_epoch
        };
        tx.commit().await?;
        Ok(ShareMutationReceipt {
            document_id: document,
            access_epoch: u64::try_from(epoch).map_err(|_| ShareError::Corrupt)?,
            newly_committed: changed,
        })
    }

    pub async fn share_document(&self, link_id: Uuid) -> Result<Option<DocumentId>, ShareError> {
        sqlx::query_scalar::<_, Uuid>("SELECT document_id FROM share_links WHERE link_id=$1")
            .bind(link_id)
            .fetch_optional(&self.pool)
            .await?
            .map(document_id)
            .transpose()
    }

    /// The server owns its Argon permit before calling this method. No workbook
    /// delivery gate, maintenance fence or document/link row lock is held here.
    pub async fn begin_share_attempt(
        &self,
        link_id: Uuid,
        token_hash: [u8; 32],
    ) -> Result<ShareAttempt<'_>, ShareError> {
        let mut tx = self.pool.begin().await?;
        let digest = Sha256::digest(link_id.as_bytes());
        let lock_key = i32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock($1,$2)")
            .bind(ATTEMPT_NAMESPACE)
            .bind(lock_key)
            .fetch_one(&mut *tx)
            .await?;
        if !acquired {
            return Err(ShareError::Conflict);
        }
        let row = fetch_link(&mut tx, link_id, false).await?;
        let mut phc = None;
        let mut frozen = None;
        if let Some(row) = row {
            let token_valid = equal_bytes(&row.token_hash, &token_hash);
            if token_valid
                && row.status == "active"
                && !row.expired
                && row.blocked_remaining_seconds == 0
            {
                let plaintext = self
                    .open_record(
                        document_id(row.document_id)?,
                        1,
                        "share-secret",
                        link_id,
                        1,
                        &row.secret_phc,
                        MAX_PHC_BYTES,
                    )
                    .await?;
                let text = std::str::from_utf8(&plaintext).map_err(|_| ShareError::Corrupt)?;
                validate_phc(text).map_err(|_| ShareError::Corrupt)?;
                phc = Some(plaintext);
            }
            frozen = Some(FrozenAttempt {
                document_id: document_id(row.document_id)?,
                token_valid,
                credential_fingerprint: Sha256::digest(&row.secret_phc).into(),
            });
        }
        Ok(ShareAttempt {
            db: self,
            tx,
            link_id,
            token_hash,
            frozen,
            phc,
        })
    }

    pub async fn guest_session(
        &self,
        link_id: Uuid,
        token_hash: [u8; 32],
    ) -> Result<Option<GuestSession>, ShareError> {
        let query = format!("{SESSION_SELECT} AND s.link_id=$1 AND s.token_hash=$2");
        sqlx::query_as::<_, SessionRow>(AssertSqlSafe(query))
            .bind(link_id)
            .bind(token_hash.as_slice())
            .fetch_optional(&self.pool)
            .await?
            .map(SessionRow::session)
            .transpose()
    }

    pub async fn expired_share_documents(&self, limit: u32) -> Result<Vec<DocumentId>, ShareError> {
        if !(1..=MAX_LIST).contains(&limit) {
            return Err(ShareError::InvalidInput);
        }
        let rows=sqlx::query_scalar::<_,Uuid>("SELECT DISTINCT document_id FROM share_links WHERE status='active' AND expires_at<=clock_timestamp() ORDER BY document_id LIMIT $1")
            .bind(i64::from(limit)).fetch_all(&self.pool).await?;
        rows.into_iter().map(document_id).collect()
    }

    /// The server holds this document's delivery gate across the durable receipt.
    pub async fn expire_document_shares(
        &self,
        document: DocumentId,
    ) -> Result<Option<ShareMutationReceipt>, ShareError> {
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let _doc = lock_document(&mut tx, document).await?;
        let query = format!(
            "{LINK_SELECT} WHERE document_id=$1 AND status='active' AND expires_at<=clock_timestamp() ORDER BY link_id FOR UPDATE"
        );
        let links = sqlx::query_as::<_, LinkRow>(AssertSqlSafe(query))
            .bind(id(document.as_bytes()))
            .fetch_all(&mut *tx)
            .await?;
        let mut epoch = None;
        for link in links {
            epoch = Some(terminal_link(&mut tx, &link, "expired", "expired", None, None).await?);
        }
        tx.commit().await?;
        epoch
            .map(|epoch| {
                Ok(ShareMutationReceipt {
                    document_id: document,
                    access_epoch: u64::try_from(epoch).map_err(|_| ShareError::Corrupt)?,
                    newly_committed: true,
                })
            })
            .transpose()
    }

    pub async fn owner_notifications(
        &self,
        identity: AuthenticatedActor,
        limit: u32,
    ) -> Result<Vec<ShareOwnerNotification>, ShareError> {
        if !(1..=MAX_LIST).contains(&limit) {
            return Err(ShareError::InvalidInput);
        }
        let rows=sqlx::query_as::<_,(Uuid,Uuid,Uuid,String,i64)>("SELECT n.notification_id,n.document_id,n.link_id,n.reason,floor(extract(epoch FROM n.created_at))::bigint FROM share_owner_notifications n JOIN users u ON u.user_id=n.user_id AND u.identity_kind='member' JOIN memberships m ON m.user_id=n.user_id JOIN document_permissions p ON p.user_id=n.user_id AND p.workspace_id=m.workspace_id AND p.document_id=n.document_id AND p.role='owner' WHERE n.user_id=$1 AND m.workspace_id=$2 ORDER BY n.created_at DESC,n.notification_id DESC LIMIT $3")
            .bind(id(identity.actor_id.as_bytes())).bind(id(identity.workspace_id.as_bytes())).bind(i64::from(limit)).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(
                |(notification_id, document, link_id, reason, created_at_unix)| {
                    Ok(ShareOwnerNotification {
                        notification_id,
                        document_id: document_id(document)?,
                        link_id,
                        reason,
                        created_at_unix,
                    })
                },
            )
            .collect()
    }
}

struct FrozenAttempt {
    document_id: DocumentId,
    token_valid: bool,
    credential_fingerprint: [u8; 32],
}
pub struct ShareAttempt<'a> {
    db: &'a PgDatabase,
    tx: Transaction<'a, Postgres>,
    link_id: Uuid,
    token_hash: [u8; 32],
    frozen: Option<FrozenAttempt>,
    phc: Option<Zeroizing<Vec<u8>>>,
}
impl ShareAttempt<'_> {
    pub fn document_id(&self) -> Option<DocumentId> {
        self.frozen.as_ref().map(|value| value.document_id)
    }
    pub fn phc(&self) -> Option<&str> {
        self.phc
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    }

    /// Called only after Argon completes and the server obtains the document
    /// delivery gate. Mutations join the shared maintenance fence at this point.
    pub async fn complete(
        mut self,
        verified: bool,
        seed: Option<GuestSessionSeed>,
    ) -> Result<ShareAttemptOutcome, ShareError> {
        self.db.guard_transaction(&mut self.tx).await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *self.tx)
            .await?;
        let Some(frozen) = self.frozen.as_ref() else {
            self.tx.commit().await?;
            return Ok(ShareAttemptOutcome::Invalid);
        };
        let doc = lock_document(&mut self.tx, frozen.document_id).await?;
        let Some(link) = fetch_link(&mut self.tx, self.link_id, true).await? else {
            self.tx.commit().await?;
            return Ok(ShareAttemptOutcome::Invalid);
        };
        if doc.state != "active"
            || link.document_id != doc.document_id
            || link.workspace_id != doc.workspace_id
            || !frozen.token_valid
            || !equal_bytes(&link.token_hash, &self.token_hash)
            || !equal_bytes(
                Sha256::digest(&link.secret_phc).as_slice(),
                &frozen.credential_fingerprint,
            )
            || link.status != "active"
        {
            if !frozen.token_valid && link.status == "active" && !link.expired {
                audit(
                    &mut self.tx,
                    &link,
                    None,
                    None,
                    "authentication_failed",
                    doc.access_epoch,
                )
                .await?;
            }
            self.tx.commit().await?;
            return Ok(ShareAttemptOutcome::Invalid);
        }
        if link.expired {
            let epoch =
                terminal_link(&mut self.tx, &link, "expired", "expired", None, None).await?;
            self.tx.commit().await?;
            return invalidated(frozen.document_id, epoch);
        }
        if link.blocked_remaining_seconds > 0 {
            let remaining_seconds =
                u64::try_from(link.blocked_remaining_seconds).map_err(|_| ShareError::Corrupt)?;
            self.tx.commit().await?;
            return Ok(ShareAttemptOutcome::Blocked { remaining_seconds });
        }
        if self.phc.is_none() {
            self.tx.commit().await?;
            return Ok(ShareAttemptOutcome::Invalid);
        }
        if verified {
            let seed = seed.ok_or(ShareError::InvalidInput)?;
            if seed.session_id.is_nil() {
                return Err(ShareError::InvalidInput);
            }
            sqlx::query("INSERT INTO users(user_id,identity_kind) VALUES($1,'share_guest')")
                .bind(id(seed.actor_id.as_bytes()))
                .execute(&mut *self.tx)
                .await?;
            let inserted=sqlx::query("WITH t AS (SELECT clock_timestamp() AS now) INSERT INTO share_guest_sessions(session_id,link_id,document_id,user_id,token_hash,admitted_head_version,created_at,expires_at) SELECT $1,l.link_id,l.document_id,$3,$4,$5,t.now,LEAST(t.now+interval '24 hours',l.expires_at) FROM share_links l CROSS JOIN t WHERE l.link_id=$2 AND l.status='active' AND (l.expires_at IS NULL OR l.expires_at>t.now)")
                .bind(seed.session_id).bind(self.link_id).bind(id(seed.actor_id.as_bytes())).bind(seed.token_hash.as_slice()).bind(doc.head_version).execute(&mut *self.tx).await?.rows_affected();
            if inserted != 1 {
                let epoch =
                    terminal_link(&mut self.tx, &link, "expired", "expired", None, None).await?;
                self.tx.commit().await?;
                return invalidated(frozen.document_id, epoch);
            }
            sqlx::query("UPDATE share_links SET failed_attempts=0,lockouts=0,blocked_until=NULL WHERE link_id=$1").bind(self.link_id).execute(&mut *self.tx).await?;
            audit(
                &mut self.tx,
                &link,
                Some(id(seed.actor_id.as_bytes())),
                None,
                "authenticated",
                doc.access_epoch,
            )
            .await?;
            let query = format!("{SESSION_SELECT} AND s.session_id=$1");
            let session = sqlx::query_as::<_, SessionRow>(AssertSqlSafe(query))
                .bind(seed.session_id)
                .fetch_optional(&mut *self.tx)
                .await?;
            let Some(session) = session else {
                let epoch =
                    terminal_link(&mut self.tx, &link, "expired", "expired", None, None).await?;
                self.tx.commit().await?;
                return invalidated(frozen.document_id, epoch);
            };
            let session = session.session()?;
            self.tx.commit().await?;
            return Ok(ShareAttemptOutcome::Accepted(session));
        }
        let failures = link
            .failed_attempts
            .checked_add(1)
            .ok_or(ShareError::Corrupt)?;
        let lockouts = if failures == 5 {
            link.lockouts.checked_add(1).ok_or(ShareError::Corrupt)?
        } else {
            link.lockouts
        };
        let updated=sqlx::query("UPDATE share_links SET failed_attempts=$2,lockouts=$3,blocked_until=CASE WHEN $4 THEN clock_timestamp()+interval '15 minutes' ELSE NULL END WHERE link_id=$1 AND status='active' AND (expires_at IS NULL OR expires_at>clock_timestamp())")
            .bind(self.link_id).bind(if failures==5{0}else{failures}).bind(lockouts).bind(failures==5&&lockouts<3).execute(&mut *self.tx).await?.rows_affected();
        if updated != 1 {
            let epoch =
                terminal_link(&mut self.tx, &link, "expired", "expired", None, None).await?;
            self.tx.commit().await?;
            return invalidated(frozen.document_id, epoch);
        }
        audit(
            &mut self.tx,
            &link,
            None,
            None,
            "authentication_failed",
            doc.access_epoch,
        )
        .await?;
        let outcome = if failures == 5 && lockouts == 3 {
            let epoch =
                terminal_link(&mut self.tx, &link, "disabled", "disabled", None, None).await?;
            invalidated(frozen.document_id, epoch)?
        } else if failures == 5 {
            audit(&mut self.tx, &link, None, None, "blocked", doc.access_epoch).await?;
            ShareAttemptOutcome::Blocked {
                remaining_seconds: 900,
            }
        } else {
            ShareAttemptOutcome::Invalid
        };
        self.tx.commit().await?;
        Ok(outcome)
    }
}

fn invalidated(document: DocumentId, epoch: i64) -> Result<ShareAttemptOutcome, ShareError> {
    Ok(ShareAttemptOutcome::Invalidated(ShareMutationReceipt {
        document_id: document,
        access_epoch: u64::try_from(epoch).map_err(|_| ShareError::Corrupt)?,
        newly_committed: true,
    }))
}

fn journal_error(error: ShareError) -> JournalError {
    match error {
        ShareError::Unauthorized | ShareError::NotFound => JournalError::Unauthorized,
        ShareError::Corrupt => JournalError::Corrupt,
        ShareError::Conflict => JournalError::Conflict,
        _ => JournalError::Unavailable,
    }
}

/// Invoked by the journal only after checking the immutable user identity kind.
/// The document lock precedes its link and session locks, matching revocation.
pub(crate) async fn guest_access_locked(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    document: DocumentId,
) -> Result<DocumentAccess, JournalError> {
    let doc = lock_document(tx, document).await.map_err(journal_error)?;
    if doc.state != "active" || doc.workspace_id != id(identity.workspace_id.as_bytes()) {
        return Err(JournalError::Unauthorized);
    }
    let link = sqlx::query_scalar::<_, Uuid>(
        "SELECT link_id FROM share_guest_sessions WHERE user_id=$1 AND document_id=$2",
    )
    .bind(id(identity.actor_id.as_bytes()))
    .bind(doc.document_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(ShareError::from)
    .map_err(journal_error)?
    .ok_or(JournalError::Unauthorized)?;
    let row = fetch_link(tx, link, true)
        .await
        .map_err(journal_error)?
        .ok_or(JournalError::Unauthorized)?;
    if row.status != "active" || row.expired {
        return Err(JournalError::Unauthorized);
    }
    let query = format!("{SESSION_SELECT} AND s.user_id=$1 AND s.document_id=$2 FOR SHARE OF s");
    let session = sqlx::query_as::<_, SessionRow>(AssertSqlSafe(query))
        .bind(id(identity.actor_id.as_bytes()))
        .bind(doc.document_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(ShareError::from)
        .map_err(journal_error)?
        .ok_or(JournalError::Unauthorized)?
        .session()
        .map_err(journal_error)?;
    Ok(DocumentAccess {
        head: document_version(doc.head_version).map_err(journal_error)?,
        document_access_epoch: u64::try_from(doc.access_epoch)
            .map_err(|_| JournalError::Corrupt)?,
        membership_access_epoch: 0,
        can_edit: session.role == ShareRole::Edit,
    })
}

/// Read authorization while taking shared row locks that fence a concurrent
/// share revocation or document access-epoch change until payload delivery ends.
pub(crate) async fn guest_delivery_access_locked(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    document: DocumentId,
) -> Result<DocumentAccess, JournalError> {
    let doc = sqlx::query_as::<_, DocumentRow>(
        "SELECT document_id,workspace_id,head_version,access_epoch,state FROM documents WHERE document_id=$1 FOR SHARE",
    )
    .bind(id(document.as_bytes()))
    .fetch_optional(&mut **tx)
    .await
    .map_err(ShareError::from)
    .map_err(journal_error)?
    .ok_or(JournalError::NotFound)?;
    if doc.state != "active" || doc.workspace_id != id(identity.workspace_id.as_bytes()) {
        return Err(JournalError::Unauthorized);
    }
    let query = format!("{SESSION_SELECT} AND s.user_id=$1 AND s.document_id=$2 FOR SHARE OF l,s");
    let session = sqlx::query_as::<_, SessionRow>(AssertSqlSafe(query))
        .bind(id(identity.actor_id.as_bytes()))
        .bind(doc.document_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(ShareError::from)
        .map_err(journal_error)?
        .ok_or(JournalError::Unauthorized)?
        .session()
        .map_err(journal_error)?;
    Ok(DocumentAccess {
        head: document_version(doc.head_version).map_err(journal_error)?,
        document_access_epoch: u64::try_from(doc.access_epoch)
            .map_err(|_| JournalError::Corrupt)?,
        membership_access_epoch: 0,
        can_edit: session.role == ShareRole::Edit,
    })
}

pub(crate) async fn guest_command_floor_locked(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    document: DocumentId,
    base: DocumentVersion,
) -> Result<(), JournalError> {
    let query =
        format!("{SESSION_SELECT} AND s.user_id=$1 AND s.document_id=$2 AND l.workspace_id=$3");
    let session = sqlx::query_as::<_, SessionRow>(AssertSqlSafe(query))
        .bind(id(identity.actor_id.as_bytes()))
        .bind(id(document.as_bytes()))
        .bind(id(identity.workspace_id.as_bytes()))
        .fetch_optional(&mut **tx)
        .await
        .map_err(ShareError::from)
        .map_err(journal_error)?
        .ok_or(JournalError::Unauthorized)?;
    if base.get()
        < u64::try_from(session.admitted_head_version).map_err(|_| JournalError::Corrupt)?
    {
        Err(JournalError::Unauthorized)
    } else {
        Ok(())
    }
}

/// The restore caller owns the exclusive maintenance fence and its transaction.
pub(crate) async fn invalidate_document_shares_after_restore(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
) -> Result<Option<u64>, ShareError> {
    sqlx::query("SET LOCAL synchronous_commit=on")
        .execute(&mut **tx)
        .await?;
    // Restored clusters may contain orphan DEKs with no document row. The
    // share-link FK makes such a document incapable of having guest access.
    let query = format!(
        "{LINK_SELECT} WHERE document_id=$1 AND status='active' ORDER BY link_id FOR UPDATE"
    );
    let links = sqlx::query_as::<_, LinkRow>(AssertSqlSafe(query))
        .bind(id(document.as_bytes()))
        .fetch_all(&mut **tx)
        .await?;
    let mut epoch = None;
    for link in links {
        epoch = Some(terminal_link(tx, &link, "revoked", "restored", None, None).await?);
    }
    sqlx::query("UPDATE share_guest_sessions SET revoked_at=clock_timestamp() WHERE document_id=$1 AND revoked_at IS NULL").bind(id(document.as_bytes())).execute(&mut **tx).await?;
    epoch
        .map(|value| u64::try_from(value).map_err(|_| ShareError::Corrupt))
        .transpose()
}

#[cfg(test)]
mod tests;
