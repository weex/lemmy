use crate::PerformCrud;
use actix_web::web::Data;
use lemmy_api_common::{
  blocking,
  get_local_user_view_from_jwt,
  person::{EditPrivateMessage, PrivateMessageResponse},
};
use lemmy_apub::activities::{
  private_message::create_or_update::CreateOrUpdatePrivateMessage,
  CreateOrUpdateType,
};
use lemmy_db_queries::{source::private_message::PrivateMessage_, Crud, DeleteableOrRemoveable};
use lemmy_db_schema::source::private_message::PrivateMessage;
use lemmy_db_views::{local_user_view::LocalUserView, private_message_view::PrivateMessageView};
use lemmy_utils::{utils::remove_slurs, ApiError, ConnectionId, LemmyError};
use lemmy_websocket::{messages::SendUserRoomMessage, LemmyContext, UserOperationCrud};

#[async_trait::async_trait(?Send)]
impl PerformCrud for EditPrivateMessage {
  type Response = PrivateMessageResponse;

  async fn perform(
    &self,
    context: &Data<LemmyContext>,
    websocket_id: Option<ConnectionId>,
  ) -> Result<PrivateMessageResponse, LemmyError> {
    let data: &EditPrivateMessage = self;
    let local_user_view = get_local_user_view_from_jwt(&data.auth, context.pool()).await?;

    // Checking permissions
    let private_message_id = data.private_message_id;
    let orig_private_message = blocking(context.pool(), move |conn| {
      PrivateMessage::read(conn, private_message_id)
    })
    .await??;
    if local_user_view.person.id != orig_private_message.creator_id {
      return Err(ApiError::err("no_private_message_edit_allowed").into());
    }

    // Doing the update
    let content_slurs_removed = remove_slurs(&data.content);
    let private_message_id = data.private_message_id;
    let updated_private_message = blocking(context.pool(), move |conn| {
      PrivateMessage::update_content(conn, private_message_id, &content_slurs_removed)
    })
    .await?
    .map_err(|_| ApiError::err("couldnt_update_private_message"))?;

    // Send the apub update
    CreateOrUpdatePrivateMessage::send(
      &updated_private_message,
      &local_user_view.person,
      CreateOrUpdateType::Update,
      context,
    )
    .await?;

    let private_message_id = data.private_message_id;
    let mut private_message_view = blocking(context.pool(), move |conn| {
      PrivateMessageView::read(conn, private_message_id)
    })
    .await??;

    // Blank out deleted or removed info
    if private_message_view.private_message.deleted {
      private_message_view.private_message = private_message_view
        .private_message
        .blank_out_deleted_or_removed_info();
    }

    let res = PrivateMessageResponse {
      private_message_view,
    };

    // Send notifications to the local recipient, if one exists
    let recipient_id = orig_private_message.recipient_id;
    if let Ok(local_recipient) = blocking(context.pool(), move |conn| {
      LocalUserView::read_person(conn, recipient_id)
    })
    .await?
    {
      let local_recipient_id = local_recipient.local_user.id;
      context.chat_server().do_send(SendUserRoomMessage {
        op: UserOperationCrud::EditPrivateMessage,
        response: res.clone(),
        local_recipient_id,
        websocket_id,
      });
    }

    Ok(res)
  }
}
