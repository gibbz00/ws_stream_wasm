use
{
	crate :: { import::*, WsErr, WsErrKind, JsMsgEvent, WsMessage, WsState, future_event },
};


/// A wrapper around [web_sys::WebSocket](https://docs.rs/web-sys/0.3.25/web_sys/struct.WebSocket.html) to make it more rust idiomatic.
/// It does not provide any extra functionality over the wrapped WebSocket object.
///
/// It turns the callback based mechanisms into futures Sink and Stream. The stream yields [JsMsgEvent], which is a wrapper
/// around [`web_sys::MessageEvent`](https://docs.rs/web-sys/0.3.25/web_sys/struct.MessageEvent.html) and the sink takes a
/// [WsMessage] which is a wrapper around  [`web_sys::MessageEvent.data()`](https://docs.rs/web-sys/0.3.25/web_sys/struct.MessageEvent.html#method.data).
/// There is no error when the server is not running, and no timeout mechanism provided here to detect that connection
/// never happens. The connect future will just never resolve.
///
/// ## Example
///
/// ```
/// #![ feature( async_await, await_macro, futures_api )]
///
/// use
/// {
///    futures::prelude      ::* ,
///    wasm_bindgen::prelude ::* ,
///    wasm_bindgen_futures  ::* ,
///    wasm_websocket_stream ::* ,
///    log                   ::* ,
/// };
///
/// let fut = async
/// {
///    let ws = WsIo::new( URL ).expect_throw( "Could not create websocket" );
///
///    ws.connect().await;
///
///    let (mut tx, mut rx) = ws.split();
///    let message          = "Hello from browser".to_string();
///
///
///    tx.send( WsMessage::Text( message.clone() )).await
///
///       .expect_throw( "Failed to write to websocket" );
///
///
///    let msg    = rx.next().await;
///    let result = &msg.expect_throw( "Stream closed" );
///
///    assert_eq!( WsMessage::Text( message ), result.data() );
///
///    Ok(())
///
/// }.boxed().compat();
///
/// spawn_local( fut );
/// ```
///
#[ allow( dead_code ) ] // we keep the closure to keep it form being dropped
//
pub struct WsIo
{
	ws     : WebSocket                                      ,
	on_mesg: Closure< dyn FnMut( MessageEvent ) + 'static > ,
	queue  : Rc<RefCell< VecDeque<JsMsgEvent> >>            ,
	waker  : Rc<RefCell<Option<Waker>>>                     , // TODO: can we use a reference rather than cloning?
}


impl WsIo
{
	/// Create a new WsIo.
	//
	pub fn new( ws: WebSocket ) -> Self
	{
		let waker: Rc<RefCell<Option<Waker>>> = Rc::new( RefCell::new( None ));

		let queue = Rc::new( RefCell::new( VecDeque::new() ) );
		let q2    = queue.clone();
		let w2    = waker.clone();


		// Send the incoming ws messages to the WsStream object
		//
		let on_mesg = Closure::wrap( Box::new( move |msg_evt: MessageEvent|
		{
			trace!( "WsStream: message received!" );

			q2.borrow_mut().push_back( WsMessage::from( &JsMsgEvent{ msg_evt } ) );

			if let Some( w ) = w2.borrow_mut().take()
			{
				trace!( "WsStream: waking up task" );
				w.wake()
			}

		}) as Box< dyn FnMut( MessageEvent ) > );


		// Install callback
		//
		ws.set_onmessage  ( Some( on_mesg.as_ref().unchecked_ref() ) );


		Self
		{
			ws      ,
			queue   ,
			on_mesg ,
			waker   ,
		}
	}



	/// Verify the [WsReadyState] of the connection.
	/// TODO: verify error handling
	//
	pub fn ready_state( &self ) -> WsState
	{
		self.ws.ready_state().try_into().map_err( |e| error!( "{}", e ) ).unwrap_throw()
	}



	// This method allows to do async close in the poll_close of Sink
	//
	async fn wake_on_close( ws: WebSocket, waker: Waker )
	{
		future_event( |cb| ws.set_onclose( cb ) ).await;

		waker.wake();
	}
}



impl fmt::Debug for WsIo
{
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result
	{
		write!( f, "WsIo" )
	}
}



impl fmt::Display for WsIo
{
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result
	{
		write!( f, "WsIo" )
	}
}



impl Drop for WsIo
{
	fn drop( &mut self )
	{
		trace!( "Drop WsIo" );

		self.ws.close().expect( "WsIo::drop - close ws socket" );
	}
}



impl Stream for WsIo
{
	type Item = WsMessage;

	// Forward the call to the channel on which we are listening.
	//
	// Currently requires an unfortunate copy from Js memory to Wasm memory. Hopefully one
	// day we will be able to receive the JsMsgEvent directly in Wasm.
	//
	fn poll_next( mut self: Pin<&mut Self>, cx: &mut Context ) -> Poll<Option< Self::Item >>
	{
		trace!( "WsIo as Stream gets polled" );

		// Once the queue is empty, check the state of the connection.
		// When it is closing or closed, no more messages will arrive, so
		// return Poll::Ready( None )
		//
		if self.queue.borrow().is_empty()
		{
			*self.waker.borrow_mut() = Some( cx.waker().clone() );

			match self.ready_state()
			{
				WsState::Open       => Poll::Pending        ,
				WsState::Connecting => Poll::Pending        ,
				_                   => Poll::Ready  ( None ),
			}
		}

		// As long as there is things in the queue, just keep reading
		//
		else { Poll::Ready( self.queue.borrow_mut().pop_front() ) }
	}
}





impl Sink<WsMessage> for WsIo
{
	type Error = WsErr;


	// Web api does not really seem to let us check for readiness, other than the connection state.
	//
	fn poll_ready( self: Pin<&mut Self>, _: &mut Context ) -> Poll<Result<(), Self::Error>>
	{
		trace!( "Sink<WsMessage> for WsIo: poll_ready" );

		match self.ready_state()
		{
			WsState::Connecting => Poll::Pending        ,
			WsState::Open       => Poll::Ready( Ok(()) ),
			_                   => Poll::Ready( Err( WsErrKind::ConnectionClosed.into() )),
		}
	}


	fn start_send( self: Pin<&mut Self>, item: WsMessage ) -> Result<(), Self::Error>
	{
		trace!( "Sink<WsMessage> for WsIo: start_send" );

		match self.ready_state()
		{
			WsState::Connecting => Err( WsErrKind::ConnectionNotReady.into() ),
			WsState::Open       =>
			{
				// TODO: fix the unwrap once web-sys can return errors: https://github.com/rustwasm/wasm-bindgen/issues/1286
				//
				match item
				{
					WsMessage::Binary( mut d ) => { self.ws.send_with_u8_array( &mut d ).unwrap(); }
					WsMessage::Text  (     s ) => { self.ws.send_with_str     ( &    s ).unwrap(); }
				}

				Ok(())
			},

			// Closing or Closed
			//
			_ => Err( WsErrKind::ConnectionClosed.into() ),
		}
	}



	fn poll_flush( self: Pin<&mut Self>, _: &mut Context ) -> Poll<Result<(), Self::Error>>
	{
		trace!( "Sink<WsMessage> for WsIo: poll_flush" );

		Poll::Ready( Ok(()) )
	}



	// TODO: find a simpler implementation, notably this needs to clone the websocket and spawn a future.
	//
	fn poll_close( self: Pin<&mut Self>, cx: &mut Context ) -> Poll<Result<(), Self::Error>>
	{
		trace!( "Sink<WsMessage> for WsIo: poll_close" );

		let state = self.ready_state();


		if state == WsState::Connecting
		|| state == WsState::Open
		{
			self.ws.close().unwrap_throw();
		}


		match state
		{
			WsState::Closed =>
			{
				trace!( "WebSocket connection closed!" );
				Poll::Ready( Ok(()) )
			}

			_ =>
			{
				rt::spawn_local( Self::wake_on_close( self.ws.clone(), cx.waker().clone() ) ).expect( "spawn wake_on_close" );
				Poll::Pending
			}
		}
	}
}





// #[ cfg(test) ]
// //
// mod test
// {
// 	wasm_bindgen_test_configure!(run_in_browser);

// 	use
// 	{
// 		crate::WsStream     ,
// 		super::*            ,
// 		wasm_bindgen_test::*,
// 		futures      :: { future::{ FutureExt, TryFutureExt }, sink::SinkExt  } ,
// 		futures_01   :: { Future as Future01                                  } ,
// 		web_sys      :: { console::log_1 as dbg                               } ,
// 	};


// 	const URL: &str = "ws://127.0.0.1:3212/";

// 	#[ wasm_bindgen_test(async) ]
// 	//
// 	fn error() -> impl Future01<Item = (), Error = JsValue>
// 	{
// 		info!( "starting test: error" );

// 		async
// 		{
// 			let (ws, mut wsio) = WsStream::connect( URL ).await.expect_throw( "Could not create websocket" );

// 			ws.close().await;


// 			let message          = "Hello from browser".to_string();

// 			let res = wsio.send( WsMessage::Text( message.clone() ) ).await;

// 			dbg( &format!( "{:?}", &res ).into() );

// 			Ok(())

// 		}.boxed_local().compat()
// 	}
// }
