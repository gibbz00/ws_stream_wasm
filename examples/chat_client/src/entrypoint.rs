#![ feature( async_await ) ]
#![ allow( unused_imports, unused_variables ) ]

mod e_handler;
mod color    ;



mod import
{
	pub(crate) use
	{
		chat_format    :: { futures_serde_cbor::{ Codec, Error }, Wire, ServerMsg, ClientMsg } ,
		async_runtime  :: { *                                                                } ,
		web_sys        :: { *, console::log_1 as dbg                                         } ,
		ws_stream_wasm :: { *                                                                } ,
		futures_codec  :: { *                                                                } ,
		futures        :: { prelude::*, stream::SplitStream                                  } ,
		futures        :: { channel::mpsc, ready                                             } ,
		log            :: { *                                                                } ,
		web_sys        :: { *                                                                } ,
		wasm_bindgen   :: { prelude::*, JsCast                                               } ,
		gloo_events    :: { *                                                                } ,
		std            :: { task::*, pin::Pin, collections::HashMap, panic                   } ,
		regex          :: { Regex                                                            } ,
		js_sys         :: { Math                                                             } ,
	};
}

use crate::{ import::*, e_handler::*, color::* };



const URL: &str = "ws://127.0.0.1:3412";


// Called when the wasm module is instantiated
//
#[ wasm_bindgen( start ) ]
//
pub fn main() -> Result<(), JsValue>
{
	panic::set_hook(Box::new(console_error_panic_hook::hook));

	wasm_logger::init( wasm_logger::Config::new(Level::Debug).message_on_new_line() );

	// Since there is no threads in wasm for the moment, this is optional if you include async_runtime
	// with `default-dependencies = false`, the local pool will be the default. However this might
	// change in the future.
	//
	rt::init( RtConfig::Local ).expect( "Set default executor" );

	let program = async
	{
		let chat = document().get_element_by_id( "chat" ).expect( "find chat"       );

		let (ws, wsio) = match WsStream::connect( URL, None ).await
		{
			Ok(conn) => conn,
			Err(e)   =>
			{
				error!( "{}", e );
				return;
			}
		};

		let framed      = Framed::new( wsio, Codec::new() );
		let (out, msgs) = framed.split();

		let send    = document().get_element_by_id( "chat_submit" ).expect_throw( "find chat_submit" );
		let form    = document().get_element_by_id( "chat_form"   ).expect_throw( "find chat_form"   );
		let tarea   = document().get_element_by_id( "chat_input"  ).expect_throw( "find chat_input"  );

		let on_send  = EHandler::new( &form , "submit"  , false );
		let on_enter = EHandler::new( &tarea, "keypress", false );

		rt::spawn_local( on_msg   ( msgs          ) ).expect( "spawn on_msg"    );
		rt::spawn_local( on_submit( on_send , out ) ).expect( "spawn on_submit" );
		rt::spawn_local( on_key   ( on_enter      ) ).expect( "spawn on_key"    );
	};

	rt::spawn_local( program ).expect( "spawn program" );

	Ok(())
}


fn append_line( document: &Document, chat: &Element, line: &str, nick: &str, color: &Color, color_all: bool )
{
	let p: HtmlElement = document.create_element( "p"    ).expect( "Failed to create div" ).unchecked_into();
	let s: HtmlElement = document.create_element( "span" ).expect( "Failed to create div" ).unchecked_into();
	let t: HtmlElement = document.create_element( "span" ).expect( "Failed to create div" ).unchecked_into();

	debug!( "setting color to: {}", color.to_css() );

	s.style().set_property( "color", &color.to_css() ).expect_throw( "set color" );

	if color_all
	{
		t.style().set_property( "color", &color.to_css() ).expect_throw( "set color" );
	}


	s.set_inner_text( &( nick.to_string() + ": " ) );
	t.set_inner_text( line );
	s.set_class_name( "nick" );
	t.set_class_name( "message_text" );

	p.append_child( &s ).expect( "Coundn't append child" );
	p.append_child( &t ).expect( "Coundn't append child" );
	chat.append_child( &p ).expect( "Coundn't append child" );

	p.scroll_into_view();
}


async fn on_msg( mut stream: impl Stream<Item=Result<Wire, Error>> + Unpin )
{
	let chat = document().get_element_by_id( "chat" ).expect_throw( "find chat"       );
	let re   = Regex::new( r"^#(\w{8})(.*)$" );

	let mut colors: HashMap<usize, Color> = HashMap::new();

	// for the server messages
	//
	colors.insert( 0, Color::random().light() );


	while let Some( msg ) = stream.next().await
	{
		// TODO: handle io errors
		//
		let msg = match msg
		{
			Ok( Wire::Server( msg ) ) => msg,
			_                         => continue,
		};



		debug!( "received message" );


		match msg
		{
			ServerMsg::ChatMsg{ nick, sid, txt } =>
			{
				if ! colors.contains_key( &sid ) { colors.insert( sid, Color::random().light() ); }

				append_line( &document(), &chat, &txt, &nick, colors.get( &sid ).unwrap(), false );
			}


			ServerMsg::ServerMsg(txt) =>
			{
				append_line( &document(), &chat, &txt, "Server", colors.get( &0 ).unwrap(), true );
			}

			_ => {}
		}
	}
   }


async fn on_submit
(
	mut submits: impl Stream< Item=Event        > + Unpin ,
	mut out    : impl Sink  < Wire, Error=Error > + Unpin ,
)
{
	let chat     = document().get_element_by_id( "chat" ).expect_throw( "find chat"       );
	let nickre   = Regex::new(r"^/nick (\w{1,15})").unwrap();
	let textarea = document().get_element_by_id( "chat_input" ).expect_throw( "find chat_input" );
	let textarea: &HtmlTextAreaElement = textarea.unchecked_ref();


	while let Some( evt ) = submits.next().await
	{
		debug!( "on_submit" );

		evt.prevent_default();

		let text = textarea.value().trim().to_string() + "\n";
		textarea.set_value( "" );
		let _ = textarea.focus();

		if text == "\n" { continue; }

		let msg;

		// if this is a /nick somename message
		//
		if let Some( cap ) = nickre.captures( &text )
		{
			debug!( "handle set nick: {:#?}", &text );

			msg = ClientMsg::SetNick( cap[1].to_string() );
		}


		else
		{
			debug!( "handle send: {:#?}", &text );

			msg = ClientMsg::ChatMsg( text );
		}



		match out.send( Wire::Client( msg ) ).await
		{
			Ok(()) => {}
			Err(e) => { error!( "{}", e ); }
		};
	};
}




// When the user presses the Enter key in the textarea we submit, rather than adding a new line
// for newline, the user can use shift+Enter.
//
// We use the click effect on the Send button, because form.submit() wont let our on_submit handler run.
//
async fn on_key
(
	mut keys: impl Stream< Item=Event > + Unpin ,
)
{
	let send: HtmlElement = document().get_element_by_id( "chat_submit" ).expect_throw( "find chat_submit" ).unchecked_into();


	while let Some( evt ) = keys.next().await
	{
		let evt: KeyboardEvent = evt.unchecked_into();

		if  evt.code() == "Enter"  &&  !evt.shift_key()
		{
			send.click();
			evt.prevent_default();
		}
	};
}



fn document() -> Document
{
	let window = web_sys::window().expect_throw( "no global `window` exists");

	window.document().expect_throw( "should have a document on window" )
}

