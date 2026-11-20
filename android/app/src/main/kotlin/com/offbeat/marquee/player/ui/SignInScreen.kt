package com.offbeat.marquee.player.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import com.offbeat.marquee.player.session.SignInForm

@Composable
fun SignInScreen(
    form: SignInForm,
    onUrl: (String) -> Unit,
    onUser: (String) -> Unit,
    onPassword: (String) -> Unit,
    onSignIn: () -> Unit,
) {
    BoxWithConstraints(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) {
        if (maxWidth >= 600.dp) {
            // Wide (landscape, foldables): the title on the left, the form on the right.
            Row(
                Modifier.fillMaxSize().padding(horizontal = 32.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(48.dp),
            ) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(12.dp)) { Header() }
                Column(
                    Modifier.weight(1f).widthIn(max = 420.dp).verticalScroll(rememberScrollState()).padding(vertical = 16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) { Fields(form, onUrl, onUser, onPassword, onSignIn) }
            }
        } else {
            Box(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), contentAlignment = Alignment.Center) {
                Column(
                    Modifier.widthIn(max = 420.dp).fillMaxWidth().padding(24.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Header()
                    Spacer(Modifier.height(4.dp))
                    Fields(form, onUrl, onUser, onPassword, onSignIn)
                }
            }
        }
    }
}

@Composable
private fun Header() {
    Text("Marquee Player", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.SemiBold)
    Text(
        "Sign in to your Navidrome or Subsonic server.",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun Fields(
    form: SignInForm,
    onUrl: (String) -> Unit,
    onUser: (String) -> Unit,
    onPassword: (String) -> Unit,
    onSignIn: () -> Unit,
) {
    var showPassword by rememberSaveable { mutableStateOf(false) }
    OutlinedTextField(
        value = form.url,
        onValueChange = onUrl,
        label = { Text("Server address") },
        placeholder = { Text("https://music.example.com") },
        singleLine = true,
        enabled = !form.busy,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next, autoCorrectEnabled = false),
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(
        value = form.user,
        onValueChange = onUser,
        label = { Text("Username") },
        singleLine = true,
        enabled = !form.busy,
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next, autoCorrectEnabled = false),
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(
        value = form.password,
        onValueChange = onPassword,
        label = { Text("Password") },
        singleLine = true,
        enabled = !form.busy,
        visualTransformation = if (showPassword) VisualTransformation.None else PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onSignIn() }),
        trailingIcon = {
            // Out of the key focus chain, so Enter from a hardware keyboard can't land on it.
            TextButton(
                onClick = { showPassword = !showPassword },
                modifier = Modifier.focusProperties { canFocus = false },
            ) { Text(if (showPassword) "Hide" else "Show") }
        },
        modifier = Modifier.fillMaxWidth(),
    )
    if (form.error != null) {
        Text(
            form.error,
            color = MaterialTheme.colorScheme.error,
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
        )
    }
    Button(
        onClick = onSignIn,
        enabled = !form.busy,
        modifier = Modifier.fillMaxWidth().height(48.dp),
    ) {
        if (form.busy) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text("Signing in…")
            }
        } else {
            Text("Sign in")
        }
    }
    Text(
        "Your password isn't stored: the app keeps a salted token, encrypted on this phone.",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.outline,
    )
}
