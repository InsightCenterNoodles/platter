//! NOODLES methods for the platter server

use colabrodo_common::client_communication::*;
use colabrodo_common::common::strings;
use colabrodo_common::components::MethodArg;
use colabrodo_common::value_tools::*;
use colabrodo_server::server::make_method_function;
use colabrodo_server::server_messages::*;
use colabrodo_server::server_state::*;

use crate::platter_state::PlatterState;
use crate::platter_state::PlatterStatePtr;
use crate::scene::Scene;

use std::sync::Arc;
use std::sync::Mutex;

use nalgebra::Quaternion;

// ================

/// Get an entity given an invocation
fn get_entity(
    context: Option<InvokeIDType>,
    state: &ServerState,
) -> Result<EntityReference, MethodException> {
    if let Some(InvokeIDType::Entity(id)) = context {
        return state
            .entities
            .resolve(id)
            .ok_or_else(|| MethodException::method_not_found(None));
    }
    Err(MethodException::method_not_found(None))
}

// ================

/// Given an invocation context, resolve to a Scene
fn get_object<'a>(
    app: &'a mut PlatterState,
    state: &ServerState,
    context: Option<InvokeIDType>,
) -> Result<&'a mut Scene, MethodException> {
    let reference = get_entity(context, state)?;
    app.find_id(&reference)
        .and_then(|id| app.get_object_mut(id))
        .ok_or_else(|| MethodException::internal_error(None))
}

/// Trait to clean up user-provided data
trait Sanitize {
    fn sanitize(self) -> Self;
}

impl<T, const N: usize> Sanitize for [T; N]
where
    T: num_traits::Float + Default,
{
    fn sanitize(self) -> Self {
        self.map(|f| {
            // Some clients provide nans...
            if f.is_nan() {
                return T::default();
            }
            f
        })
    }
}

// =============================================================================

make_method_function!(set_position,
    PlatterState,
    strings::MTHD_SET_POSITION,
    "Set the position of an entity.",
    |position : [f32;3] : "New position of entity, as vec3"|,
    {
        let obj = get_object(app, state, context)?;

        obj.set_position(position.sanitize().into());

        Ok(None)
    }
);

make_method_function!(set_rotation,
    PlatterState,
    strings::MTHD_SET_ROTATION,
    "Set the rotation of an entity.",
    |quaternion : [f32;4] : "New rotation of entity, as vec4"|,
    {
        let obj = get_object(app, state, context)?;

        let q = quaternion.sanitize();

        obj.set_rotation(Quaternion::new(q[3], q[0], q[1], q[2]));

        Ok(None)
    }
);

make_method_function!(set_scale,
    PlatterState,
    strings::MTHD_SET_SCALE,
    "Set the scale of an entity.",
    |scale : [f32;3] : "New scaling of entity, as vec3"|,
    {
        let obj = get_object(app, state, context)?;

        obj.set_scale(scale.sanitize().into());

        Ok(None)
    }
);

pub fn setup_methods(state: ServerStatePtr, app_state: PlatterStatePtr) -> Vec<MethodReference> {
    let mut lock = state.lock().unwrap();

    let ret = vec![
        lock.methods
            .new_owned_component(create_set_position(app_state.clone())),
        lock.methods
            .new_owned_component(create_set_rotation(app_state.clone())),
        lock.methods
            .new_owned_component(create_set_scale(app_state)),
    ];

    ret
}
